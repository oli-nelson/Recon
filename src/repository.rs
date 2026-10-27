use anyhow::{Context, Result, bail};
use git2::{ObjectType, Repository, StatusOptions};
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
        if entry.kind() == Some(ObjectType::Commit) {
            return Ok(Content {
                kind: Kind::Submodule,
                size: 0,
                hash: *blake3::hash(entry.id().to_string().as_bytes()).as_bytes(),
                text: Some(entry.id().to_string().into()),
                mode: 0o160000,
                modified_at: None,
            });
        }
        if entry.kind() != Some(ObjectType::Blob) {
            return Ok(Content::missing());
        }
        let odb = self.git.odb()?;
        let (size, _) = odb.read_header(entry.id())?;
        if size as u64 > self.max_text {
            // Stream large HEAD blobs instead of allocating their entire contents.
            let mut child = std::process::Command::new("git")
                .arg("-C")
                .arg(&self.root)
                .args(["cat-file", "blob", &entry.id().to_string()])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn()?;
            let mut stream = child.stdout.take().context("Git blob stream unavailable")?;
            let mut hasher = blake3::Hasher::new();
            let mut buf = [0u8; 65536];
            loop {
                if self.cancel.load(Ordering::Relaxed) {
                    let _ = child.kill();
                    let _ = child.wait();
                    bail!("Cancelled");
                }
                let n = stream.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
            }
            if !child.wait()?.success() {
                bail!("Failed to read HEAD blob for {path}");
            }
            return Ok(Content {
                kind: Kind::Large,
                size: size as u64,
                hash: *hasher.finalize().as_bytes(),
                text: None,
                mode: entry.filemode() as u32,
                modified_at: None,
            });
        }
        let blob = self.git.find_blob(entry.id())?;
        Ok(Content::from_bytes(
            blob.content(),
            entry.filemode() as u32,
            self.max_text,
        ))
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
        if meta.len() > self.max_text {
            let mut hasher = blake3::Hasher::new();
            let mut buf = [0u8; 65536];
            let mut size = 0;
            loop {
                if self.cancel.load(Ordering::Relaxed) {
                    bail!("Cancelled");
                }
                let n = file.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                size += n as u64;
            }
            return Ok(Content {
                kind: Kind::Large,
                size,
                hash: *hasher.finalize().as_bytes(),
                text: None,
                mode,
                modified_at,
            });
        }
        let mut bytes = Vec::with_capacity(meta.len() as usize);
        file.by_ref()
            .take(self.max_text.saturating_add(1))
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > self.max_text {
            let mut hasher = blake3::Hasher::new();
            hasher.update(&bytes);
            let mut size = bytes.len() as u64;
            let mut buf = [0u8; 65536];
            loop {
                if self.cancel.load(Ordering::Relaxed) {
                    bail!("Cancelled");
                }
                let n = file.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                size += n as u64;
            }
            return Ok(Content {
                kind: Kind::Large,
                size,
                hash: *hasher.finalize().as_bytes(),
                text: None,
                mode,
                modified_at,
            });
        }
        Ok(Content::from_bytes(&bytes, mode, self.max_text).with_modified_at(modified_at))
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
