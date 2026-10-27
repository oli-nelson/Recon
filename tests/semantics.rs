use clap::Parser;
use recon::{
    Config,
    engine::Engine,
    model::{Model, Status, Update},
    watch::Worker,
};
use std::{
    collections::BTreeSet,
    fs,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};
use tempfile::TempDir;

fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
fn fixture() -> TempDir {
    let d = tempfile::tempdir().unwrap();
    git(d.path(), &["init", "-q"]);
    git(d.path(), &["config", "user.email", "test@example.invalid"]);
    git(d.path(), &["config", "user.name", "Recon Test"]);
    fs::write(d.path().join("a.txt"), "one\n").unwrap();
    fs::write(d.path().join(".gitignore"), "*.ignored\n").unwrap();
    git(d.path(), &["add", "."]);
    git(d.path(), &["commit", "-qm", "initial"]);
    d
}
fn setup(d: &TempDir) -> (Engine, Model) {
    let config = Config::parse_from(["recon", d.path().to_str().unwrap()]);
    (
        Engine::new(&config).unwrap(),
        Model::new(1000, 64 * 1024 * 1024),
    )
}
fn reset(e: &mut Engine, m: &mut Model, clear: bool) {
    e.reset(clear, "test", &mut |u| m.apply(u)).unwrap();
}
fn refresh(e: &mut Engine, m: &mut Model) {
    e.refresh(None, false, &mut |u| m.apply(u)).unwrap();
}
#[test]
fn edits_undo_delete_recreate_clear_and_git_reset() {
    let d = fixture();
    let (mut e, mut m) = setup(&d);
    fs::write(d.path().join("a.txt"), "two\n").unwrap();
    reset(&mut e, &mut m, false);
    assert_eq!(m.entries.len(), 1);
    assert!(m.entries[0].diff.contains("-one"));
    fs::write(d.path().join("a.txt"), "three\n").unwrap();
    refresh(&mut e, &mut m);
    assert_eq!(m.entries.len(), 2);
    assert!(m.entries[1].diff.contains("-two"));
    fs::write(d.path().join("a.txt"), "one\n").unwrap();
    refresh(&mut e, &mut m);
    assert!(m.entries[2].diff.contains("-three"));
    assert_eq!(m.files["a.txt"].status, Status::Unchanged);
    fs::remove_file(d.path().join("a.txt")).unwrap();
    refresh(&mut e, &mut m);
    assert_eq!(m.files["a.txt"].status, Status::Deleted);
    fs::write(d.path().join("a.txt"), "four\n").unwrap();
    refresh(&mut e, &mut m);
    assert_eq!(m.files["a.txt"].status, Status::Modified);
    assert_eq!(m.entries.len(), 5);
    reset(&mut e, &mut m, true);
    assert!(m.files.is_empty());
    refresh(&mut e, &mut m);
    assert!(m.files.is_empty());
    fs::write(d.path().join("a.txt"), "five\n").unwrap();
    refresh(&mut e, &mut m);
    assert!(m.entries[0].diff.contains("-four"));
    fs::write(d.path().join("a.txt"), "four\n").unwrap();
    refresh(&mut e, &mut m);
    assert_eq!(m.files["a.txt"].status, Status::Unchanged);
    reset(&mut e, &mut m, false);
    assert_eq!(m.entries.len(), 1);
    assert!(m.entries[0].diff.contains("-one"));
}
#[test]
fn stage_only_no_duplicate_and_cancelled_index_explained() {
    let d = fixture();
    let (mut e, mut m) = setup(&d);
    reset(&mut e, &mut m, false);
    fs::write(d.path().join("a.txt"), "two\n").unwrap();
    refresh(&mut e, &mut m);
    git(d.path(), &["add", "a.txt"]);
    refresh(&mut e, &mut m);
    assert_eq!(m.entries.len(), 1);
    assert_eq!(m.files["a.txt"].git, "M ");
    fs::write(d.path().join("a.txt"), "one\n").unwrap();
    reset(&mut e, &mut m, false);
    assert_eq!(m.files["a.txt"].status, Status::Unchanged);
    assert_eq!(m.files["a.txt"].git, "MM");
    assert!(m.entries[0].diff.contains("cancel"));
}
#[test]
fn new_files_ignored_and_notifications_without_content_edits() {
    let d = fixture();
    let (mut e, mut m) = setup(&d);
    reset(&mut e, &mut m, true);
    fs::write(d.path().join("new.txt"), "new\n").unwrap();
    fs::write(d.path().join("noise.ignored"), "secret\n").unwrap();
    refresh(&mut e, &mut m);
    assert_eq!(m.files.len(), 1);
    assert_eq!(m.files["new.txt"].status, Status::New);
    fs::write(d.path().join("new.txt"), "changed\n").unwrap();
    refresh(&mut e, &mut m);
    assert_eq!(m.files["new.txt"].status, Status::New);
    assert_eq!(m.entries.len(), 2);
    let paths: BTreeSet<_> = ["new.txt".into()].into();
    e.refresh(Some(paths), false, &mut |u| m.apply(u)).unwrap();
    assert_eq!(m.entries.len(), 2);
}
#[test]
fn unborn_repo_binary_symlink_and_head_boundary() {
    let d = tempfile::tempdir().unwrap();
    git(d.path(), &["init", "-q"]);
    let (mut e, mut m) = setup(&d);
    fs::write(d.path().join("binary"), [0, 1, 2]).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("/etc/passwd", d.path().join("link")).unwrap();
    reset(&mut e, &mut m, false);
    assert!(m.entries.iter().any(|e| e.diff.contains("Binary")));
    #[cfg(unix)]
    assert!(m.entries.iter().any(|e| e.diff.contains("+/etc/passwd")));
    git(d.path(), &["config", "user.email", "test@example.invalid"]);
    git(d.path(), &["config", "user.name", "Recon Test"]);
    git(d.path(), &["add", "."]);
    git(d.path(), &["commit", "-qm", "initial"]);
    reset(&mut e, &mut m, true);
    refresh(&mut e, &mut m);
    assert_eq!(m.baseline, "last clear");
    fs::write(d.path().join("next"), "next").unwrap();
    git(d.path(), &["add", "."]);
    git(d.path(), &["commit", "-qm", "next"]);
    refresh(&mut e, &mut m);
    assert_eq!(m.baseline, "HEAD");
    assert!(m.message.contains("HEAD changed"));
    assert!(m.entries.is_empty());
}
#[test]
fn reconciliation_recovers_missed_events_and_marks_gap() {
    let d = fixture();
    let (mut e, mut m) = setup(&d);
    reset(&mut e, &mut m, false);
    fs::write(d.path().join("a.txt"), "missed\n").unwrap();
    e.refresh(None, true, &mut |u| m.apply(u)).unwrap();
    assert_eq!(m.entries.len(), 1);
    assert!(m.message.contains("History gap"));
    refresh(&mut e, &mut m);
    assert_eq!(m.entries.len(), 1);
}
fn drain_until(worker: &Worker, mut predicate: impl FnMut(Update) -> bool) {
    let end = Instant::now() + Duration::from_secs(8);
    while Instant::now() < end {
        if let Ok(u) = worker.updates.recv_timeout(Duration::from_millis(100)) {
            if predicate(u) {
                return;
            }
        }
    }
    panic!("timed out waiting for watcher update");
}
#[test]
fn native_watcher_observes_atomic_save_and_burst_final_contents() {
    let d = fixture();
    let config = Config::parse_from([
        "recon",
        d.path().to_str().unwrap(),
        "--reconcile-seconds",
        "1",
    ]);
    let worker = Worker::spawn(config).unwrap();
    drain_until(&worker, |u| matches!(u, Update::Done));
    for i in 0..20 {
        fs::write(d.path().join("tmp"), format!("burst {i}\n")).unwrap();
        fs::rename(d.path().join("tmp"), d.path().join("a.txt")).unwrap();
    }
    drain_until(&worker, |u| {
        if let Update::Files(rows) = u {
            rows.iter().any(|(r, e)| {
                r.path == "a.txt" && e.as_ref().is_some_and(|e| e.diff.contains("+burst 19"))
            })
        } else {
            false
        }
    });
    worker.stop();
}
#[test]
fn linked_worktree_discovery() {
    let d = fixture();
    let other = tempfile::tempdir().unwrap();
    let path = other.path().join("worktree");
    git(
        d.path(),
        &["worktree", "add", "-qb", "linked", path.to_str().unwrap()],
    );
    fs::write(path.join("a.txt"), "linked\n").unwrap();
    let config = Config::parse_from(["recon", path.join("a.txt").to_str().unwrap()]);
    let mut engine = Engine::new(&config).unwrap();
    let mut m = Model::new(10, 10240);
    engine.reset(false, "test", &mut |u| m.apply(u)).unwrap();
    assert!(m.entries[0].diff.contains("+linked"));
}

#[test]
fn renamed_paths_and_text_budget_summaries_are_explicit() {
    let d = fixture();
    git(d.path(), &["mv", "a.txt", "renamed.txt"]);
    let (mut e, mut m) = setup(&d);
    reset(&mut e, &mut m, false);
    assert!(m.files.values().any(|r| r.git.contains("rename")));
    assert!(m.entries.iter().any(|e| e.activity.contains("renamed")));
    let config = Config::parse_from([
        "recon",
        d.path().to_str().unwrap(),
        "--snapshot-bytes",
        "1024",
    ]);
    let mut e = Engine::new(&config).unwrap();
    fs::write(d.path().join("budget.txt"), "baseline\n".repeat(2000)).unwrap();
    reset(&mut e, &mut m, true);
    fs::write(d.path().join("budget.txt"), "different\n").unwrap();
    refresh(&mut e, &mut m);
    assert!(m.entries.iter().any(|e| e.diff.contains("budget exceeded")));
}

#[test]
fn size_limits_and_custom_ignores() {
    let d = fixture();
    let config = Config::parse_from([
        "recon",
        d.path().to_str().unwrap(),
        "--max-text-bytes",
        "8",
        "--ignore",
        "skip/**",
    ]);
    fs::write(d.path().join("a.txt"), "much longer than eight bytes\n").unwrap();
    fs::create_dir(d.path().join("skip")).unwrap();
    fs::write(d.path().join("skip/noise"), "noise").unwrap();
    let mut e = Engine::new(&config).unwrap();
    let mut m = Model::new(10, 10240);
    reset(&mut e, &mut m, false);
    assert!(!m.files.contains_key("skip/noise"));
    assert!(m.entries.iter().any(|e| e.diff.contains("Large")));
    refresh(&mut e, &mut m);
    assert_eq!(m.entries.len(), 1);
}

#[test]
fn failed_clear_keeps_the_previous_baseline_and_worker_can_stop_while_busy() {
    let d = fixture();
    let (mut e, mut m) = setup(&d);
    reset(&mut e, &mut m, false);
    e.repo
        .cancel
        .store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(e.reset(true, "test", &mut |u| m.apply(u)).is_err());
    assert_eq!(m.baseline, "HEAD");
    for i in 0..200 {
        fs::write(d.path().join(format!("pending{i}")), "pending\n").unwrap();
    }
    let config = Config::parse_from(["recon", d.path().to_str().unwrap()]);
    let worker = Worker::spawn(config).unwrap();
    let start = Instant::now();
    worker.stop();
    assert!(start.elapsed() < Duration::from_secs(2));
}

#[test]
fn embedded_repositories_are_opaque() {
    let d = fixture();
    let nested = d.path().join("nested");
    fs::create_dir(&nested).unwrap();
    git(&nested, &["init", "-q"]);
    fs::write(nested.join("inside.txt"), "inside\n").unwrap();
    let (mut e, mut m) = setup(&d);
    reset(&mut e, &mut m, false);
    assert!(m.files.contains_key("nested"));
    assert!(m.files["nested"].note.contains("nested repository"));
    assert!(!m.files.contains_key("nested/inside.txt"));
    let entries = m.entries.len();
    fs::write(nested.join("inside.txt"), "changed\n").unwrap();
    e.refresh(Some(["nested/inside.txt".into()].into()), false, &mut |u| {
        m.apply(u)
    })
    .unwrap();
    assert_eq!(m.entries.len(), entries);
}
