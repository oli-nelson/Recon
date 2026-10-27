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
            repo,
            baseline: None,
            observed: BTreeMap::new(),
            statuses: BTreeMap::new(),
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
        self.observed.clear();
        self.baseline = captured;
        self.used = captured_bytes;
        self.head = head;
        self.branch = branch.clone();
        self.statuses = statuses;
        emit(Update::Reset {
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
    fn base(&self, path: &str) -> Result<Content> {
        if let Some(base) = &self.baseline {
            Ok(base.get(path).cloned().unwrap_or_else(Content::missing))
        } else {
            self.repo.head_content(path, self.head)
        }
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
                "HEAD changed • automatic fresh Git snapshot; previous baseline/history discarded",
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
