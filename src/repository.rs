use anyhow::{Context, Result, bail};
use git2::{AttrCheckFlags, AttrValue, Repository, StatusOptions};
use globset::{Glob, GlobSet, GlobSetBuilder};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Missing,
    Text,
    Binary,
    Large,
    Symlink,
    Submodule,
    Unreadable,
}
#[derive(Debug, Clone)]
pub struct Content {
    pub kind: Kind,
    pub size: u64,
    pub hash: [u8; 32],
    pub text: Option<Arc<str>>,
    pub mode: u32,
    pub modified_at: Option<std::time::SystemTime>,
}
impl Content {
    pub fn missing() -> Self {
        Self {
            kind: Kind::Missing,
            size: 0,
            hash: [0; 32],
            text: None,
            mode: 0,
            modified_at: None,
        }
    }
    pub fn with_modified_at(mut self, time: Option<std::time::SystemTime>) -> Self {
        self.modified_at = time;
        self
    }
    pub fn same(&self, other: &Self) -> bool {
        self.kind == other.kind
            && self.hash == other.hash
            && self.size == other.size
            && self.mode == other.mode
    }
    pub fn bytes(&self) -> usize {
        self.text.as_ref().map_or(0, |s| s.len())
    }
    pub fn summary(&self) -> String {
        format!(
            "{:?}, {} bytes{}",
            self.kind,
            self.size,
            if self.text.is_none() && self.kind == Kind::Text {
                " (snapshot text budget exceeded)"
            } else {
                ""
            }
        )
    }
    pub fn from_bytes(bytes: &[u8], mode: u32, limit: u64) -> Self {
        let hash = *blake3::hash(bytes).as_bytes();
        let kind = if mode == 0o120000 {
            Kind::Symlink
        } else if bytes.len() as u64 > limit {
            Kind::Large
        } else if bytes.contains(&0) || std::str::from_utf8(bytes).is_err() {
            Kind::Binary
        } else {
            Kind::Text
        };
        let text = if matches!(kind, Kind::Text | Kind::Symlink) {
            std::str::from_utf8(bytes).ok().map(Arc::from)
        } else {
            None
        };
        Self {
            kind,
            hash,
            text,
            size: bytes.len() as u64,
            mode,
            modified_at: None,
        }
    }
}

pub struct Repo {
    pub git: Repository,
    pub root: PathBuf,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
    pub ignore: GlobSet,
    pub max_text: u64,
    pub cancel: Arc<AtomicBool>,
    pub submodules: BTreeSet<String>,
}
impl Repo {
    pub fn unpushed_commits(&self) -> Result<crate::model::UnpushedCommits> {
        use crate::model::UnpushedCommits;
        let unavailable = |message: &str| UnpushedCommits {
            titles: Vec::new(),
            unavailable: Some(message.into()),
        };
        let head = match self.git.head() {
            Ok(head) => head,
            Err(error)
                if matches!(
                    error.code(),
                    git2::ErrorCode::UnbornBranch | git2::ErrorCode::NotFound
                ) =>
            {
                return Ok(unavailable("No commits yet"));
            }
            Err(error) => return Err(error.into()),
        };
        if !head.is_branch() {
            return Ok(unavailable("Detached HEAD"));
        }
        let branch = git2::Branch::wrap(head);
        let mut walk = self.git.revwalk()?;
        walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)?;
        walk.push(branch.get().peel_to_commit()?.id())?;
        match branch.upstream() {
            Ok(upstream) => walk.hide(upstream.get().peel_to_commit()?.id())?,
            Err(error) if error.code() == git2::ErrorCode::NotFound => {
                // Match `git log HEAD --not --remotes` when no upstream exists.
                walk.hide_glob("refs/remotes/*")?;
            }
            Err(error) => return Err(error.into()),
        }
        let mut titles = Vec::new();
        for id in walk {
            if self.cancel.load(Ordering::Relaxed) {
                bail!("Cancelled");
            }
            let commit = self.git.find_commit(id?)?;
            titles.push(
                String::from_utf8_lossy(commit.summary_bytes().unwrap_or(b"(no title)"))
                    .into_owned(),
            );
        }
        titles.reverse();
        Ok(UnpushedCommits {
            titles,
            unavailable: None,
        })
    }

    pub fn discover(path: &Path, ignores: &[String], max_text: u64) -> Result<Self> {
        let path = if path.is_file() {
            path.parent().unwrap_or(path)
        } else {
            path
        };
        let git = Repository::discover(path).context("No Git repository found")?;
        let root = git
            .workdir()
            .context("Recon requires a working tree (bare repositories are unsupported)")?
            .canonicalize()?;
        let mut builder = GlobSetBuilder::new();
        for pattern in ignores {
            builder.add(
                Glob::new(pattern).with_context(|| format!("Invalid ignore glob: {pattern}"))?,
            );
        }
        let submodules = git
            .submodules()?
            .iter()
            .map(|s| s.path().to_string_lossy().into_owned())
            .collect();
        let git_dir = git.path().canonicalize()?;
        let common_dir = git.commondir().canonicalize()?;
        Ok(Self {
            git_dir,
            common_dir,
            git,
            root,
            ignore: builder.build()?,
            max_text,
            cancel: Arc::new(AtomicBool::new(false)),
            submodules,
        })
    }
    pub fn branch(&self) -> Result<String> {
        match self.git.head() {
            Ok(head) if head.is_branch() => {
                Ok(head.shorthand().unwrap_or("unknown branch").to_owned())
            }
            Ok(head) => Ok(format!(
                "detached {}",
                head.target()
                    .map(|id| id.to_string()[..8].to_owned())
                    .unwrap_or_else(|| "HEAD".into())
            )),
            Err(error)
                if matches!(
                    error.code(),
                    git2::ErrorCode::UnbornBranch | git2::ErrorCode::NotFound
                ) =>
            {
                let reference = self.git.find_reference("HEAD")?;
                let name = reference
                    .symbolic_target()
                    .unwrap_or("HEAD")
                    .strip_prefix("refs/heads/")
                    .unwrap_or("HEAD");
                Ok(format!("{name} (unborn)"))
            }
            Err(error) => Err(error.into()),
        }
    }
    pub fn head(&self) -> Result<Option<git2::Oid>> {
        match self.git.head() {
            Ok(r) => Ok(Some(r.peel_to_commit()?.id())),
            Err(e)
                if matches!(
                    e.code(),
                    git2::ErrorCode::UnbornBranch | git2::ErrorCode::NotFound
                ) =>
            {
                Ok(None)
            }
            Err(e) => Err(e.into()),
        }
    }
    pub fn excluded(&self, path: &str) -> bool {
        path.split('/').any(|s| s == ".git")
            || self.ignore.is_match(path)
            || Path::new(path)
                .ancestors()
                .skip(1)
                .any(|p| self.ignore.is_match(p))
            || self
                .submodules
                .iter()
                .any(|p| path.starts_with(&format!("{p}/")))
    }
    pub fn ignored(&self, path: &str) -> bool {
        self.excluded(path)
            || self
                .git
                .status_should_ignore(Path::new(path))
                .unwrap_or(false)
    }
    pub fn statuses(&self, paths: Option<&BTreeSet<String>>) -> Result<BTreeMap<String, String>> {
        let mut opts = StatusOptions::new();
        opts.include_untracked(true)
            .recurse_untracked_dirs(true)
            .renames_head_to_index(true)
            .renames_index_to_workdir(false)
            .include_ignored(false);
        if let Some(paths) = paths {
            opts.disable_pathspec_match(true);
            for path in paths {
                opts.pathspec(path);
            }
        }
        let statuses = self.git.statuses(Some(&mut opts))?;
        let mut result = BTreeMap::new();
        for e in statuses.iter() {
            let path = std::str::from_utf8(e.path_bytes())
                .context("Non-UTF-8 Git path is unsupported")?
                .trim_end_matches('/');
            if self.excluded(path) {
                continue;
            }
            let s = e.status();
            let index = if s.is_index_new() {
                'A'
            } else if s.is_index_deleted() {
                'D'
            } else if s.is_index_renamed() {
                'R'
            } else if s.is_index_typechange() {
                'T'
            } else if s.is_index_modified() {
                'M'
            } else {
                ' '
            };
            let work = if s.is_wt_new() {
                '?'
            } else if s.is_wt_deleted() {
                'D'
            } else if s.is_wt_typechange() {
                'T'
            } else if s.is_wt_modified() {
                'M'
            } else {
                ' '
            };
            let mut label = if s.is_conflicted() {
                "UU".into()
            } else if s.is_wt_new() {
                "??".into()
            } else {
                format!("{index}{work}")
            };
            if s.is_index_renamed() {
                if let Some(delta) = e.head_to_index() {
                    if let Some(new) = delta.new_file().path().and_then(Path::to_str) {
                        label.push_str(&format!(" rename from {path}"));
                        result.insert(new.to_owned(), label);
                        // Keep the old side for a correct final-content comparison.
                        result.insert(path.to_owned(), "R  renamed away".into());
                        continue;
                    }
                }
            }
            result.insert(path.to_owned(), label);
        }
        Ok(result)
    }
    pub fn all_paths(&self) -> Result<BTreeSet<String>> {
        let mut paths = BTreeSet::new();
        for entry in self.git.index()?.iter() {
            let path =
                std::str::from_utf8(&entry.path).context("Non-UTF-8 Git path is unsupported")?;
            if !self.excluded(path) {
                paths.insert(path.to_owned());
            }
        }
        for path in self.statuses(None)?.keys() {
            paths.insert(path.clone());
        }
        Ok(paths)
    }
    pub fn head_content(&self, path: &str, head: Option<git2::Oid>) -> Result<Content> {
        let Some(oid) = head else {
            return Ok(Content::missing());
        };
        let tree = self.git.find_commit(oid)?.tree()?;
        let entry = match tree.get_path(Path::new(path)) {
            Ok(e) => e,
            Err(e) if e.code() == git2::ErrorCode::NotFound => return Ok(Content::missing()),
            Err(e) => return Err(e.into()),
        };
        let (oid, mode, normalize) = self.head_key(path, entry.id(), entry.filemode() as u32)?;
        self.blob_content(path, oid, mode, normalize)
    }
    pub(crate) fn head_key(
        &self,
        path: &str,
        oid: git2::Oid,
        mode: u32,
    ) -> Result<(git2::Oid, u32, bool)> {
        Ok((
            oid,
            self.comparison_mode(mode)?,
            mode != 0o120000 && mode != 0o160000 && self.normalize_line_endings(path)?,
        ))
    }
    pub(crate) fn blob_content(
        &self,
        path: &str,
        oid: git2::Oid,
        mode: u32,
        normalize: bool,
    ) -> Result<Content> {
        if mode == 0o160000 {
            return Ok(Content {
                kind: Kind::Submodule,
                size: 0,
                hash: *blake3::hash(oid.to_string().as_bytes()).as_bytes(),
                text: Some(oid.to_string().into()),
                mode,
                modified_at: None,
            });
        }
        if !matches!(mode, 0o100644 | 0o100755 | 0o120000) {
            return Ok(Content::missing());
        }
        let odb = self.git.odb()?;
        let (size, _) = odb.read_header(oid)?;
        if size as u64 > self.max_text {
            // Stream raw blobs: never execute repository clean/smudge filters.
            let mut child = std::process::Command::new("git")
                .arg("-C")
                .arg(&self.root)
                .args(["cat-file", "blob", &oid.to_string()])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn()?;
            let mut stream = child.stdout.take().context("Git blob stream unavailable")?;
            let result = self.read_content(&mut stream, mode, normalize);
            if result.is_err() {
                let _ = child.kill();
            }
            let status = child.wait()?;
            let content = result?;
            if !status.success() {
                bail!("Failed to read HEAD blob for {path}");
            }
            return Ok(content);
        }
        let blob = self.git.find_blob(oid)?;
        self.read_content(&mut blob.content(), mode, normalize)
    }
    fn comparison_mode(&self, mode: u32) -> Result<u32> {
        let config = self.git.config()?;
        if mode == 0o120000 && !config.get_bool("core.symlinks").unwrap_or(true) {
            return Ok(0o100644);
        }
        if matches!(mode, 0o100644 | 0o100755)
            && (!cfg!(unix) || !config.get_bool("core.filemode").unwrap_or(true))
        {
            return Ok(0o100644);
        }
        Ok(mode)
    }
    fn normalize_line_endings(&self, path: &str) -> Result<bool> {
        let config = self.git.config()?;
        if !config.get_bool("core.symlinks").unwrap_or(true)
            && self
                .git
                .index()?
                .get_path(Path::new(path), 0)
                .is_some_and(|entry| entry.mode == 0o120000)
        {
            return Ok(false);
        }
        let text = AttrValue::from_string(self.git.get_attr(
            Path::new(path),
            "text",
            AttrCheckFlags::default(),
        )?);
        if text == AttrValue::False {
            return Ok(false);
        }
        let eol = self
            .git
            .get_attr(Path::new(path), "eol", AttrCheckFlags::default())?;
        Ok(matches!(text, AttrValue::True | AttrValue::String("auto"))
            || matches!(eol, Some("lf" | "crlf"))
            || config.get_bool("core.autocrlf").unwrap_or(false)
            || config
                .get_string("core.autocrlf")
                .is_ok_and(|v| v == "input"))
    }
    /// Normalize configured UTF-8 text only; binary data and symlink targets stay raw.
    /// Fingerprints stream even when the retained text exceeds the configured limit.
    fn read_content(&self, reader: &mut impl Read, mode: u32, normalize: bool) -> Result<Content> {
        let mut raw = Fingerprint::default();
        let mut normalized = Fingerprint::default();
        let mut utf8_tail = Vec::new();
        let mut textual = normalize;
        let mut pending_cr = false;
        let mut buf = [0u8; 65536];
        loop {
            if self.cancel.load(Ordering::Relaxed) {
                bail!("Cancelled");
            }
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            let bytes = &buf[..n];
            raw.update(bytes, self.max_text);
            if textual {
                utf8_tail.extend_from_slice(bytes);
                match std::str::from_utf8(&utf8_tail) {
                    Ok(_) => utf8_tail.clear(),
                    Err(error) if error.error_len().is_none() => {
                        let tail = utf8_tail[error.valid_up_to()..].to_vec();
                        utf8_tail = tail;
                    }
                    Err(_) => textual = false,
                }
                if bytes.contains(&0) {
                    textual = false;
                }
                let mut converted = Vec::with_capacity(n + 1);
                for &byte in bytes {
                    if pending_cr && byte != b'\n' {
                        converted.push(b'\r');
                    }
                    if byte != b'\r' {
                        converted.push(byte);
                    }
                    pending_cr = byte == b'\r';
                }
                normalized.update(&converted, self.max_text);
            }
        }
        if pending_cr {
            normalized.update(b"\r", self.max_text);
        }
        let fingerprint = if textual && utf8_tail.is_empty() {
            normalized
        } else {
            raw
        };
        Ok(fingerprint.content(mode, self.max_text))
    }
    pub fn working_content(&self, path: &str) -> Result<Content> {
        // Never traverse a symlink in a parent component.
        let rel = Path::new(path);
        if rel.is_absolute()
            || rel
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            bail!("Unsafe repository path: {path}");
        }
        let mut parent = self.root.clone();
        if let Some(p) = rel.parent() {
            for component in p.components() {
                parent.push(component);
                if fs::symlink_metadata(&parent).is_ok_and(|m| m.file_type().is_symlink()) {
                    return Ok(Content::missing());
                }
            }
        }
        let full = self.root.join(rel);
        let meta = match fs::symlink_metadata(&full) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Content::missing()),
            Err(e) => return Ok(unreadable(e.to_string())),
        };
        let modified_at = meta.modified().ok();
        if meta.file_type().is_symlink() {
            let target = fs::read_link(&full)?;
            return Ok(Content::from_bytes(
                target.as_os_str().as_encoded_bytes(),
                0o120000,
                self.max_text,
            )
            .with_modified_at(modified_at));
        }
        if self.submodules.contains(path) || (meta.is_dir() && full.join(".git").exists()) {
            let text = Repository::open(&full)
                .ok()
                .and_then(|r| r.head().ok().and_then(|h| h.target()))
                .map(|id| id.to_string())
                .unwrap_or_default();
            return Ok(Content {
                kind: Kind::Submodule,
                size: 0,
                hash: *blake3::hash(text.as_bytes()).as_bytes(),
                text: Some(text.into()),
                mode: 0o160000,
                modified_at,
            });
        }
        if !meta.is_file() {
            return Ok(Content::missing());
        }
        #[cfg(unix)]
        let mode = {
            use std::os::unix::fs::PermissionsExt;
            if meta.permissions().mode() & 0o111 != 0 {
                0o100755
            } else {
                0o100644
            }
        };
        #[cfg(not(unix))]
        let mode = 0o100644;
        let mut file = match fs::File::open(&full) {
            Ok(f) => f,
            Err(e) => return Ok(unreadable(e.to_string())),
        };
        let mode = self.comparison_mode(mode)?;
        let normalize = self.normalize_line_endings(path)?;
        Ok(self
            .read_content(&mut file, mode, normalize)?
            .with_modified_at(modified_at))
    }
}
#[derive(Default)]
struct Fingerprint {
    hash: blake3::Hasher,
    size: u64,
    text: Vec<u8>,
}
impl Fingerprint {
    fn update(&mut self, bytes: &[u8], limit: u64) {
        self.hash.update(bytes);
        self.size += bytes.len() as u64;
        if self.size <= limit {
            self.text.extend_from_slice(bytes);
        } else {
            self.text.clear();
        }
    }
    fn content(self, mode: u32, limit: u64) -> Content {
        if self.size <= limit {
            Content::from_bytes(&self.text, mode, limit)
        } else {
            Content {
                kind: Kind::Large,
                size: self.size,
                hash: *self.hash.finalize().as_bytes(),
                text: None,
                mode,
                modified_at: None,
            }
        }
    }
}

fn unreadable(message: String) -> Content {
    Content {
        kind: Kind::Unreadable,
        size: 0,
        hash: *blake3::hash(message.as_bytes()).as_bytes(),
        text: Some(message.into()),
        mode: 0,
        modified_at: None,
    }
}
