use crate::{
    Config,
    model::{Entry, FileRow, Status, Update},
    repository::{Content, Kind, Repo},
};
use anyhow::Result;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

pub struct Engine {
    pub repo: Repo,
    baseline: Option<BTreeMap<String, Content>>,
    observed: BTreeMap<String, Content>,
    statuses: BTreeMap<String, String>,
    head: Option<git2::Oid>,
    head_tree: Option<git2::Oid>,
    head_paths: BTreeMap<String, Option<(git2::Oid, u32)>>,
    head_contents: BTreeMap<(git2::Oid, u32, bool), Content>,
    branch: String,
    next_id: u64,
    budget: usize,
    used: usize,
}
impl Engine {
    pub fn new(config: &Config) -> Result<Self> {
        let repo = Repo::discover(&config.path, &config.ignores, config.max_text_bytes)?;
        let head = repo.head()?;
        let branch = repo.branch()?;
        Ok(Self {
            baseline: None,
            observed: BTreeMap::new(),
            statuses: BTreeMap::new(),
            head_tree: head
                .map(|id| repo.git.find_commit(id).map(|commit| commit.tree_id()))
                .transpose()?,
            head_paths: BTreeMap::new(),
            head_contents: BTreeMap::new(),
            repo,
            head,
            branch,
            next_id: 1,
            budget: config.snapshot_bytes as usize,
            used: 0,
        })
    }
    pub fn reset(
        &mut self,
        clear: bool,
        reason: &str,
        emit: &mut impl FnMut(Update),
    ) -> Result<()> {
        // Capture before replacing state: a failed clear must leave the old baseline intact.
        let head = self.repo.head()?;
        let branch = self.repo.branch()?;
        let statuses = self.repo.statuses(None)?;
        let mut captured = None;
        let mut captured_bytes = 0;
        if clear {
            let paths = self.repo.all_paths()?;
            let total = paths.len();
            let mut baseline = BTreeMap::new();
            for (i, path) in paths.into_iter().enumerate() {
                if self.repo.cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    anyhow::bail!("Cancelled");
                }
                let mut content = self.repo.working_content(&path)?;
                if content.kind == Kind::Submodule {
                    self.repo.submodules.insert(path.clone());
                }
                if content.kind == Kind::Unreadable {
                    anyhow::bail!(
                        "Cannot establish baseline: {path}: {}",
                        content.text.as_deref().unwrap_or("unreadable")
                    );
                }
                if captured_bytes + content.bytes() > self.budget {
                    content.text = None;
                }
                captured_bytes += content.bytes();
                baseline.insert(path, content);
                if i % 250 == 0 {
                    emit(Update::Progress(format!(
                        "Capturing clear baseline {i}/{total}"
                    )));
                }
            }
            captured = Some(baseline);
        }
        let head_tree = head
            .map(|id| self.repo.git.find_commit(id).map(|commit| commit.tree_id()))
            .transpose()?;
        self.head_tree = head_tree;
        self.head_paths.clear();
        self.head_contents.clear();
        self.observed.clear();
        self.baseline = captured;
        self.used = captured_bytes;
        self.head = head;
        self.branch = branch.clone();
        self.statuses = statuses;
        emit(Update::Reset {
            root: self.repo.root.clone(),
            baseline: if clear { "last clear" } else { "HEAD" }.into(),
            reason: reason.into(),
            branch,
        });
        if clear {
            emit(Update::Notice(
                "Cleared • only subsequent activity will appear".into(),
            ));
        } else {
            let paths = self.statuses.keys().cloned().collect();
            self.update_paths(paths, true, emit)?;
        }
        emit(Update::Done);
        Ok(())
    }
    fn retain(&mut self, content: &mut Content) {
        if self.used.saturating_add(content.bytes()) > self.budget {
            content.text = None;
        }
        self.used += content.bytes();
    }
    fn base(&mut self, path: &str) -> Result<Content> {
        if let Some(base) = &self.baseline {
            return Ok(base.get(path).cloned().unwrap_or_else(Content::missing));
        }
        let identity = if let Some(identity) = self.head_paths.get(path) {
            *identity
        } else {
            let identity = if let Some(tree) = self.head_tree {
                let tree = self.repo.git.find_tree(tree)?;
                match tree.get_path(std::path::Path::new(path)) {
                    Ok(entry) => Some((entry.id(), entry.filemode() as u32)),
                    Err(error) if error.code() == git2::ErrorCode::NotFound => None,
                    Err(error) => return Err(error.into()),
                }
            } else {
                None
            };
            self.head_paths.insert(path.to_owned(), identity);
            identity
        };
        let Some((oid, mode)) = identity else {
            return Ok(Content::missing());
        };
        // Attributes and comparison config can change without changing HEAD.
        let key = self.repo.head_key(path, oid, mode)?;
        if let Some(content) = self.head_contents.get(&key) {
            return Ok(content.clone());
        }
        let mut content = self.repo.blob_content(path, key.0, key.1, key.2)?;
        self.retain(&mut content);
        self.head_contents.insert(key, content.clone());
        Ok(content)
    }

    pub(crate) fn observed_under(&self, path: &str) -> bool {
        let prefix = format!("{path}/");
        [
            &self.observed,
            self.baseline.as_ref().unwrap_or(&self.observed),
        ]
        .into_iter()
        .any(|map| {
            map.contains_key(path)
                || map
                    .range(prefix.clone()..)
                    .next()
                    .is_some_and(|(key, _)| key.starts_with(&prefix))
        }) || self.statuses.contains_key(path)
            || self
                .statuses
                .range(prefix.clone()..)
                .next()
                .is_some_and(|(key, _)| key.starts_with(&prefix))
    }
    pub fn refresh(
        &mut self,
        paths: Option<BTreeSet<String>>,
        gap: bool,
        emit: &mut impl FnMut(Update),
    ) -> Result<()> {
        if self.repo.head()? != self.head || self.repo.branch()? != self.branch {
            self.repo.submodules = self
                .repo
                .git
                .submodules()?
                .iter()
                .map(|s| s.path().to_string_lossy().into_owned())
                .collect();
            return self.reset(
                false,
                "HEAD changed • comparison refreshed; previous baseline/history discarded",
                emit,
            );
        }
        emit(Update::Progress("Refreshing…".into()));
        let reconciled = paths.is_none();
        let mut paths = if let Some(paths) = paths {
            let statuses = self.repo.statuses(Some(&paths))?;
            for path in &paths {
                self.statuses.remove(path);
            }
            self.statuses.extend(statuses);
            paths
        } else {
            let old = std::mem::replace(&mut self.statuses, self.repo.statuses(None)?);
            old.keys()
                .chain(self.statuses.keys())
                .chain(self.observed.keys())
                .cloned()
                .collect()
        };
        paths.retain(|p| !self.repo.excluded(p));
        if gap {
            emit(Update::Notice(
                "History gap • watcher lost events; reconciling observed endpoints only".into(),
            ));
        }
        let changes = self.update_paths(paths, false, emit)?;
        if reconciled && changes > 0 && !gap {
            emit(Update::Notice("Reconciled observed transitions • possible history gap; intermediate edits may be missing".into()));
        }
        emit(Update::Done);
        Ok(())
    }
    fn update_paths(
        &mut self,
        paths: BTreeSet<String>,
        opening: bool,
        emit: &mut impl FnMut(Update),
    ) -> Result<usize> {
        let total = paths.len();
        let mut changes = 0;
        let mut batch = Vec::new();
        let mut batch_bytes = 0;
        for (i, path) in paths.into_iter().enumerate() {
            if self.repo.cancel.load(std::sync::atomic::Ordering::Relaxed) {
                anyhow::bail!("Cancelled");
            }
            let base = match self.base(&path) {
                Ok(content) => content,
                Err(error) => {
                    emit(Update::Notice(format!(
                        "{path}: baseline unavailable ({error}); retrying on reconciliation"
                    )));
                    continue;
                }
            };
            // A newly ignored untracked path vanishes from the live set, but its history is retained.
            let ignored = !self.statuses.contains_key(&path)
                && base.kind == Kind::Missing
                && self.repo.ignored(&path);
            let mut current = if ignored {
                Content::missing()
            } else {
                match self.repo.working_content(&path) {
                    Ok(content) => content,
                    Err(error) => {
                        emit(Update::Notice(format!(
                            "{path}: read failed ({error}); preserving last observation; possible history gap"
                        )));
                        continue;
                    }
                }
            };
            if current.kind == Kind::Submodule {
                self.repo.submodules.insert(path.clone());
            }
            if current.kind == Kind::Unreadable {
                emit(Update::Notice(format!(
                    "{path}: inaccessible; preserving last observation and retrying during reconciliation"
                )));
                continue;
            }
            let previous = self.observed.get(&path).unwrap_or(&base);
            let changed = !previous.same(&current);
            if changed {
                changes += 1;
            }
            let net = !base.same(&current);
            let git = self
                .statuses
                .get(&path)
                .cloned()
                .unwrap_or_else(|| "  ".into());
            let status = if !net {
                Status::Unchanged
            } else if current.kind == Kind::Missing {
                Status::Deleted
            } else if base.kind == Kind::Missing {
                Status::New
            } else {
                Status::Modified
            };
            let mut note = if !net && git != "  " {
                "No net working-tree hunk; index/worktree changes cancel or status-only change"
                    .into()
            } else if !net {
                "No net change • history retained".into()
            } else {
                String::new()
            };
            if current.kind == Kind::Submodule {
                note = "Submodule / nested repository • contents are not watched".into();
            }
            let entry = if changed || (opening && git != "  ") {
                let activity = if opening && git.starts_with('R') {
                    "opening snapshot • renamed"
                } else if opening {
                    "opening snapshot"
                } else if current.kind == Kind::Missing {
                    "deleted"
                } else if previous.kind == Kind::Missing {
                    "created"
                } else if !net {
                    "reverted to baseline"
                } else {
                    "edited"
                };
                let diff = if changed {
                    make_diff(&path, previous, &current, self.repo.max_text)
                } else {
                    note.clone()
                };
                let collapsed = diff.lines().count() > 60 || diff.len() > 16 * 1024;
                let entry = Entry {
                    id: self.next_id,
                    path: path.clone(),
                    time: SystemTime::now(),
                    activity: activity.into(),
                    line_count: diff.lines().count(),
                    diff: Arc::from(diff),
                    collapsed,
                };
                self.next_id += 1;
                Some(entry)
            } else {
                None
            };
            let activity = if changed || opening {
                Instant::now()
            } else {
                Instant::now() - Duration::from_secs(10)
            };
            batch_bytes += entry.as_ref().map_or(0, Entry::bytes);
            batch.push((
                FileRow {
                    path: path.clone(),
                    status,
                    remove_from_tree: current.kind == Kind::Missing && git == "  ",
                    modified_at: current
                        .modified_at
                        .or_else(|| (current.kind == Kind::Missing).then(SystemTime::now)),
                    git,
                    note,
                    activity,
                },
                entry,
            ));
            if let Some(old) = self.observed.remove(&path) {
                self.used = self.used.saturating_sub(old.bytes());
            }
            if net {
                self.retain(&mut current);
                self.observed.insert(path, current);
            }
            if batch.len() >= 64 || batch_bytes >= 1024 * 1024 {
                batch_bytes = 0;
                emit(Update::Files(std::mem::take(&mut batch)));
                emit(Update::Progress(format!(
                    "Comparing files {}/{total}",
                    i + 1
                )));
            }
        }
        if !batch.is_empty() {
            emit(Update::Files(batch));
        }
        Ok(changes)
    }
}

fn make_diff(path: &str, old: &Content, new: &Content, limit: u64) -> String {
    let empty: Arc<str> = Arc::from("");
    let before = if old.kind == Kind::Missing {
        Some(&empty)
    } else {
        old.text.as_ref()
    };
    let after = if new.kind == Kind::Missing {
        Some(&empty)
    } else {
        new.text.as_ref()
    };
    if matches!(old.kind, Kind::Binary | Kind::Large | Kind::Submodule)
        || matches!(new.kind, Kind::Binary | Kind::Large | Kind::Submodule)
        || before.is_none()
        || after.is_none()
    {
        return format!(
            "{} → {}\nContent changed; textual diff unavailable.",
            old.summary(),
            new.summary()
        );
    }
    let diff = similar::TextDiff::configure()
        .deadline(Instant::now() + Duration::from_millis(40))
        .diff_lines(before.unwrap().as_ref(), after.unwrap().as_ref());
    let mut output = diff
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{path}"), &format!("b/{path}"))
        .to_string();
    if output.is_empty() {
        output = format!("Metadata changed: mode {:o} → {:o}", old.mode, new.mode);
    }
    let cap = (limit.saturating_mul(2)).min(4 * 1024 * 1024) as usize;
    if output.len() > cap {
        let mut end = cap;
        while !output.is_char_boundary(end) {
            end -= 1;
        }
        output.truncate(end);
        output.push_str("\n[diff truncated to text limit]");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn commit(repo: &git2::Repository, text: &[u8]) {
        let oid = repo.blob(text).unwrap();
        let mut builder = repo.treebuilder(None).unwrap();
        builder.insert("a", oid, 0o100644).unwrap();
        builder.insert("b", oid, 0o100644).unwrap();
        let tree = repo.find_tree(builder.write().unwrap()).unwrap();
        let signature = git2::Signature::now("Test", "test@example.invalid").unwrap();
        let parent = repo.head().ok().and_then(|head| head.peel_to_commit().ok());
        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            "fixture",
            &tree,
            &parent.iter().collect::<Vec<_>>(),
        )
        .unwrap();
    }

    #[test]
    fn head_cache_shares_blobs_bounds_text_and_replaces_head_mapping() {
        let dir = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        commit(&repo, b"one\r\n");
        let config = Config::parse_from(["recon", dir.path().to_str().unwrap()]);
        let mut engine = Engine::new(&config).unwrap();
        engine.budget = 5;
        let first = engine.base("a").unwrap();
        let second = engine.base("b").unwrap();
        assert!(Arc::ptr_eq(
            first.text.as_ref().unwrap(),
            second.text.as_ref().unwrap()
        ));
        assert_eq!(engine.head_contents.len(), 1);
        assert_eq!(engine.used, 5);
        // Attributes differ by path even for the same blob; fingerprints must differ.
        std::fs::write(dir.path().join(".gitattributes"), "b text\n").unwrap();
        let normalized = engine.base("b").unwrap();
        assert_ne!(normalized.hash, first.hash);
        assert!(normalized.text.is_none());
        assert_eq!(engine.used, 5);
        assert_eq!(engine.head_contents.len(), 2);
        assert!(engine.base("missing").unwrap().kind == Kind::Missing);
        commit(&repo, b"two\n");
        engine.refresh(None, false, &mut |_| {}).unwrap();
        let replacement = engine.base("a").unwrap();
        assert_ne!(replacement.hash, first.hash);
        assert!(engine.used <= engine.budget);
    }

    #[test]
    fn large_head_fingerprint_is_reused_without_retained_text() {
        let dir = tempfile::tempdir().unwrap();
        let repo = git2::Repository::init(dir.path()).unwrap();
        commit(&repo, &vec![b'x'; 1024 * 1024]);
        let mut config = Config::parse_from(["recon", dir.path().to_str().unwrap()]);
        config.max_text_bytes = 1024;
        let mut engine = Engine::new(&config).unwrap();
        let first = engine.base("a").unwrap();
        assert_eq!(first.kind, Kind::Large);
        // A new streaming read would fail cancellation; a cache hit needs no read.
        engine
            .repo
            .cancel
            .store(true, std::sync::atomic::Ordering::Relaxed);
        for _ in 0..10 {
            assert!(engine.base("a").unwrap().same(&first));
        }
        assert_eq!(engine.head_contents.len(), 1);
        assert_eq!(engine.used, 0);
    }
}
