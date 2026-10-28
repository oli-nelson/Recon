use crate::{
    Config,
    engine::Engine,
    model::{Command, Update},
};
use anyhow::Result;
use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::{
    collections::BTreeSet,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::{Duration, Instant},
};

pub struct Worker {
    pub commands: SyncSender<Command>,
    pub updates: Receiver<Update>,
    handle: Option<thread::JoinHandle<()>>,
    cancel: Arc<AtomicBool>,
}
impl Worker {
    pub fn spawn(config: Config) -> Result<Self> {
        // Discovery is cheap; all file reads and status/diff work happen on the worker.
        let engine = Engine::new(&config)?;
        let cancel = engine.repo.cancel.clone();
        let (commands, command_rx) = mpsc::sync_channel(8);
        let (tx, updates) = mpsc::sync_channel(4);
        let handle = thread::spawn(move || run(engine, config, command_rx, tx));
        Ok(Self {
            commands,
            updates,
            cancel,
            handle: Some(handle),
        })
    }
    pub fn stop(mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        let _ = self.commands.try_send(Command::Quit);
        // Drain bounded results while the worker finishes to avoid a send/join deadlock.
        drop(self.updates);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}
fn run(mut engine: Engine, config: Config, commands: Receiver<Command>, tx: SyncSender<Update>) {
    let (event_tx, events) = mpsc::sync_channel::<notify::Result<Event>>(4096);
    let overflow = Arc::new(AtomicBool::new(false));
    let flag = overflow.clone();
    let watcher = RecommendedWatcher::new(
        move |event: notify::Result<Event>| {
            // Read/access events are irrelevant; watching a diff must not watch its own reads.
            if event
                .as_ref()
                .is_ok_and(|e: &Event| matches!(e.kind, EventKind::Access(_)))
            {
                return;
            }
            if event_tx.try_send(event).is_err() {
                flag.store(true, Ordering::Relaxed);
            }
        },
        notify::Config::default().with_follow_symlinks(false),
    );
    let mut watch_notice = None;
    let mut watcher = match watcher {
        Ok(w) => Some(w),
        Err(e) => {
            watch_notice = Some(format!(
                "Watcher unavailable ({e}); periodic reconciliation active"
            ));
            None
        }
    };
    if let Some(w) = &mut watcher {
        let repo = &engine.repo;
        let root_watched = match w.watch(&repo.root, RecursiveMode::Recursive) {
            Ok(()) => true,
            Err(error) => {
                watch_notice = Some(format!(
                    "Watcher partially unavailable ({error}); periodic reconciliation active"
                ));
                false
            }
        };
        // An ordinary .git directory is already covered by the root watch. Linked
        // worktrees need separate watches, but never subscribe to the same tree twice.
        let mut watched = Vec::new();
        if root_watched {
            watched.push(&repo.root);
        }
        for metadata in [&repo.common_dir, &repo.git_dir] {
            if watched.iter().any(|path| metadata.starts_with(path)) {
                continue;
            }
            match w.watch(metadata, RecursiveMode::Recursive) {
                Ok(()) => watched.push(metadata),
                Err(error) => {
                    watch_notice = Some(format!(
                        "Git metadata watcher unavailable ({error}); periodic reconciliation active"
                    ));
                }
            }
        }
    }
    let mut disconnected = false;
    let mut emit = |u| {
        if tx.send(u).is_err() {
            disconnected = true;
        }
    };
    if let Err(e) = engine.reset(false, "", &mut emit) {
        let _ = tx.send(Update::Notice(format!("Snapshot failed: {e:#}")));
        let _ = tx.send(Update::Done);
    }
    if let Some(notice) = watch_notice {
        let _ = tx.send(Update::WatchStatus(notice));
    }
    let mut pending = BTreeSet::new();
    let mut full = false;
    let mut gap = false;
    let mut first = None;
    let mut last_event = Instant::now();
    let mut last_reconcile = Instant::now();
    loop {
        if disconnected || engine.repo.cancel.load(Ordering::Relaxed) {
            break;
        }
        match commands.try_recv() {
            Ok(Command::Quit) | Err(mpsc::TryRecvError::Disconnected) => break,
            Ok(command) => {
                let mut emit = |u| {
                    let _ = tx.send(u);
                };
                let result = match command {
                    Command::Clear => engine.reset(true, "Capturing session baseline…", &mut emit),
                    Command::Git => engine.reset(false, "Comparing against HEAD", &mut emit),
                    Command::Reconcile => engine.refresh(None, true, &mut emit),
                    Command::Quit => unreachable!(),
                };
                if let Err(e) = result {
                    let _ = tx.send(Update::Notice(format!("Refresh failed: {e:#}")));
                    let _ = tx.send(Update::Done);
                }
                // Keep events queued during capture: they may represent edits after a path was read.
                pending.clear();
                full = false;
                first = None;
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        if overflow.swap(false, Ordering::Relaxed) {
            full = true;
            gap = true;
            first.get_or_insert(Instant::now());
        }
        match events.recv_timeout(Duration::from_millis(25)) {
            Ok(Ok(event)) => {
                let mut relevant = event.need_rescan();
                if relevant {
                    full = true;
                    gap = true;
                }
                for path in event.paths {
                    match classify(&path, &engine) {
                        Some(Some(path)) => {
                            pending.insert(path);
                            relevant = true;
                        }
                        Some(None) => {
                            full = true;
                            relevant = true;
                        }
                        None => {}
                    }
                }
                if pending.len() > 4096 {
                    pending.clear();
                    full = true;
                    gap = true;
                }
                if relevant {
                    last_event = Instant::now();
                    first.get_or_insert(last_event);
                }
            }
            Ok(Err(_)) => {
                full = true;
                gap = true;
                first.get_or_insert(Instant::now());
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                // Watcher initialization can fail (e.g. Linux inotify limits).
                // A disconnected channel returns immediately instead of waiting.
                thread::sleep(Duration::from_millis(25));
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if last_reconcile.elapsed() >= Duration::from_secs(config.reconcile_seconds) {
            full = true;
            first.get_or_insert(Instant::now() - Duration::from_secs(2));
        }
        let due = first.is_some_and(|start| {
            last_event.elapsed() >= Duration::from_millis(config.debounce_ms)
                || start.elapsed() >= Duration::from_millis(250)
        });
        if due {
            let mut emit = |u| {
                if tx.send(u).is_err() {
                    disconnected = true;
                }
            };
            let paths = if full {
                None
            } else {
                Some(std::mem::take(&mut pending))
            };
            if let Err(e) = engine.refresh(paths, gap, &mut emit) {
                let _ = tx.send(Update::Notice(format!("Refresh failed: {e:#}")));
                let _ = tx.send(Update::Done);
            }
            if full {
                last_reconcile = Instant::now();
            }
            pending.clear();
            full = false;
            gap = false;
            first = None;
        }
    }
}
fn classify(raw: &Path, engine: &Engine) -> Option<Option<String>> {
    // FSEvents can report /var while discovery resolves /private/var. Resolve
    // existing parents, retaining the leaf so symlink targets are never followed.
    let mut parent = raw.parent()?;
    let mut suffix = vec![raw.file_name()?.to_os_string()];
    let canonical = loop {
        if let Ok(path) = parent.canonicalize() {
            break path;
        }
        suffix.push(parent.file_name()?.to_os_string());
        parent = parent.parent()?;
    };
    let mut normalized = canonical;
    for part in suffix.into_iter().rev() {
        normalized.push(part);
    }
    let path = normalized.as_path();
    let repo = &engine.repo;
    if path.starts_with(&repo.git_dir) || path.starts_with(&repo.common_dir) {
        // Ignore object writes and locks. Ref, HEAD, index and exclude changes affect the view.
        let name = path.file_name()?.to_str()?;
        if name == "index"
            || name == "HEAD"
            || name == "packed-refs"
            || name == "exclude"
            || path.components().any(|p| p.as_os_str() == "refs")
        {
            return Some(None);
        }
        return None;
    }
    let relative = path.strip_prefix(&repo.root).ok()?;
    let p = relative.to_str()?;
    #[cfg(windows)]
    let p = p.replace('\\', "/");
    #[cfg(not(windows))]
    let p = p.to_owned();
    if p.is_empty() || p.ends_with(".gitignore") {
        return Some(None);
    }
    if repo.excluded(&p) {
        return None;
    }
    // Directory renames/removals require discovering affected descendants.
    if path.is_dir() || (!path.exists() && !repo.submodules.contains(&p)) {
        return Some(None);
    }
    Some(Some(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[cfg(unix)]
    #[test]
    fn backslash_is_a_filename_character_on_unix() {
        let directory = tempfile::tempdir().unwrap();
        git2::Repository::init(directory.path()).unwrap();
        let config = Config::parse_from(["recon", directory.path().to_str().unwrap()]);
        let engine = Engine::new(&config).unwrap();
        let path = directory.path().join("literal\\name.txt");
        std::fs::write(&path, "content").unwrap();
        assert_eq!(
            classify(&path, &engine),
            Some(Some("literal\\name.txt".into()))
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_events_use_git_path_separators() {
        let directory = tempfile::tempdir().unwrap();
        git2::Repository::init(directory.path()).unwrap();
        let config = Config::parse_from(["recon", directory.path().to_str().unwrap()]);
        let engine = Engine::new(&config).unwrap();
        let path = directory.path().join("nested").join("file.txt");
        std::fs::create_dir(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "content").unwrap();
        assert_eq!(
            classify(&path, &engine),
            Some(Some("nested/file.txt".into()))
        );
    }
}
