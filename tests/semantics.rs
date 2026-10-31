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
fn unpushed_commits_follow_upstream_refs_and_survive_clearing_history() {
    let d = fixture();
    git(d.path(), &["branch", "published"]);
    git(d.path(), &["branch", "--set-upstream-to=published"]);
    git(
        d.path(),
        &[
            "commit",
            "--allow-empty",
            "-qm",
            "Older title\n\nBody omitted",
        ],
    );
    git(d.path(), &["commit", "--allow-empty", "-qm", "Newer title"]);
    let (mut engine, mut model) = setup(&d);
    reset(&mut engine, &mut model, false);
    assert_eq!(model.unpushed.titles, ["Older title", "Newer title"]);
    assert_eq!(model.unpushed.unavailable, None);
    reset(&mut engine, &mut model, true);
    assert_eq!(model.unpushed.titles, ["Older title", "Newer title"]);
    // Upstream changes without a HEAD change must refresh the list too.
    git(d.path(), &["update-ref", "refs/heads/published", "HEAD~1"]);
    refresh(&mut engine, &mut model);
    assert_eq!(model.unpushed.titles, ["Newer title"]);
    git(d.path(), &["update-ref", "refs/heads/published", "HEAD"]);
    refresh(&mut engine, &mut model);
    assert!(model.unpushed.titles.is_empty());
    git(d.path(), &["branch", "--unset-upstream"]);
    refresh(&mut engine, &mut model);
    assert_eq!(model.unpushed.unavailable, None);
    assert_eq!(
        model.unpushed.titles,
        ["initial", "Older title", "Newer title"]
    );
    git(d.path(), &["checkout", "--detach", "-q"]);
    refresh(&mut engine, &mut model);
    assert!(model.unpushed.titles.is_empty());
    assert_eq!(model.unpushed.unavailable.as_deref(), Some("Detached HEAD"));
}

#[test]
fn unpushed_without_upstream_excludes_all_remote_refs_and_refreshes() {
    let d = fixture();
    git(
        d.path(),
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
    );
    git(
        d.path(),
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
    );
    git(
        d.path(),
        &["commit", "--allow-empty", "-qm", "Published elsewhere"],
    );
    git(
        d.path(),
        &["update-ref", "refs/remotes/other/topic", "HEAD"],
    );
    git(d.path(), &["pack-refs", "--all"]);
    git(d.path(), &["commit", "--allow-empty", "-qm", "Local only"]);
    let (mut engine, mut model) = setup(&d);
    reset(&mut engine, &mut model, false);
    assert_eq!(model.unpushed.unavailable, None);
    assert_eq!(model.unpushed.titles, ["Local only"]);
    // Publishing changes the result even when HEAD stays the same.
    git(
        d.path(),
        &["update-ref", "refs/remotes/other/topic", "HEAD"],
    );
    refresh(&mut engine, &mut model);
    assert!(model.unpushed.titles.is_empty());
}

#[test]
fn unpushed_commits_handle_unborn_and_diverged_branches() {
    let empty = tempfile::tempdir().unwrap();
    git(empty.path(), &["init", "-q"]);
    let (mut engine, mut model) = setup(&empty);
    reset(&mut engine, &mut model, false);
    assert_eq!(
        model.unpushed.unavailable.as_deref(),
        Some("No commits yet")
    );

    let d = fixture();
    git(d.path(), &["checkout", "-qb", "published"]);
    git(
        d.path(),
        &["commit", "--allow-empty", "-qm", "Upstream only"],
    );
    git(d.path(), &["checkout", "-qb", "local", "HEAD~1"]);
    git(d.path(), &["branch", "--set-upstream-to=published"]);
    git(d.path(), &["commit", "--allow-empty", "-qm", "Local only"]);
    let (mut engine, mut model) = setup(&d);
    reset(&mut engine, &mut model, false);
    assert_eq!(model.unpushed.titles, ["Local only"]);
    git(d.path(), &["merge", "--no-edit", "published"]);
    refresh(&mut engine, &mut model);
    assert_eq!(model.unpushed.titles.len(), 2);
    assert_eq!(model.unpushed.titles[0], "Local only");
    assert!(model.unpushed.titles[1].starts_with("Merge"));
}

#[test]
fn configured_crlf_checkout_does_not_invent_changes_on_watcher_events() {
    let d = fixture();
    git(d.path(), &["config", "core.autocrlf", "true"]);
    fs::write(d.path().join("a.txt"), "one\r\n").unwrap();
    // Refresh the index stat data as a real CRLF checkout would.
    git(d.path(), &["add", "a.txt"]);
    let (mut engine, mut model) = setup(&d);
    reset(&mut engine, &mut model, false);
    engine
        .refresh(Some(BTreeSet::from(["a.txt".into()])), false, &mut |u| {
            model.apply(u)
        })
        .unwrap();
    assert!(model.files.is_empty(), "{:?}", model.files);
    assert!(model.entries.is_empty());

    fs::write(d.path().join("a.txt"), "two\r\n").unwrap();
    refresh(&mut engine, &mut model);
    assert_eq!(model.files["a.txt"].status, Status::Modified);
    assert!(model.entries.back().unwrap().diff.contains("-one\n+two\n"));
    fs::write(d.path().join("a.txt"), "one\r\n").unwrap();
    refresh(&mut engine, &mut model);
    assert!(model.files.is_empty());
}

#[test]
fn untrusted_executable_bits_do_not_invent_changes() {
    let d = fixture();
    git(d.path(), &["update-index", "--chmod=+x", "a.txt"]);
    git(d.path(), &["commit", "-qm", "executable script"]);
    git(d.path(), &["config", "core.filemode", "false"]);
    let (mut engine, mut model) = setup(&d);
    let base = engine
        .repo
        .head_content("a.txt", engine.repo.head().unwrap())
        .unwrap();
    assert!(base.same(&engine.repo.working_content("a.txt").unwrap()));
    reset(&mut engine, &mut model, false);
    engine
        .refresh(Some(BTreeSet::from(["a.txt".into()])), false, &mut |u| {
            model.apply(u)
        })
        .unwrap();
    assert!(model.files.is_empty());
    assert!(model.entries.is_empty());
    fs::write(d.path().join("a.txt"), "edited\n").unwrap();
    refresh(&mut engine, &mut model);
    assert_eq!(model.files["a.txt"].status, Status::Modified);
}

#[test]
fn disabled_symlinks_compare_plain_checkout_stubs() {
    let d = fixture();
    let repo = git2::Repository::open(d.path()).unwrap();
    let target = repo.blob(b"a.txt").unwrap().to_string();
    git(
        d.path(),
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            "120000",
            &target,
            "link",
        ],
    );
    git(d.path(), &["commit", "-qm", "symlink"]);
    git(d.path(), &["config", "core.symlinks", "false"]);
    git(d.path(), &["config", "core.autocrlf", "true"]);
    fs::write(d.path().join("link"), "a.txt").unwrap();
    let (mut engine, mut model) = setup(&d);
    let base = engine
        .repo
        .head_content("link", engine.repo.head().unwrap())
        .unwrap();
    assert!(base.same(&engine.repo.working_content("link").unwrap()));
    reset(&mut engine, &mut model, false);
    assert!(model.files.is_empty());
    fs::write(d.path().join("link"), "different-target").unwrap();
    refresh(&mut engine, &mut model);
    assert!(
        model
            .entries
            .back()
            .unwrap()
            .diff
            .contains("+different-target")
    );
}

#[test]
fn attributes_control_eol_comparison_and_binary_bytes_stay_exact() {
    for (attributes, autocrlf, equal) in [
        ("*.txt text eol=crlf\n", "false", true),
        ("*.txt text eol=lf\n", "false", true),
        ("*.txt -text\n", "true", false),
        ("", "false", false),
        ("", "input", true),
    ] {
        let d = fixture();
        git(d.path(), &["config", "core.autocrlf", autocrlf]);
        fs::write(d.path().join(".gitattributes"), attributes).unwrap();
        let (engine, _) = setup(&d);
        let base = engine
            .repo
            .head_content("a.txt", engine.repo.head().unwrap())
            .unwrap();
        fs::write(d.path().join("a.txt"), "one\r\n").unwrap();
        assert_eq!(
            base.same(&engine.repo.working_content("a.txt").unwrap()),
            equal
        );
        for bytes in [b"\0one\r\n".as_slice(), b"\xffone\r\n"] {
            fs::write(d.path().join("a.txt"), bytes).unwrap();
            assert_eq!(
                engine.repo.working_content("a.txt").unwrap().hash,
                *blake3::hash(bytes).as_bytes()
            );
        }
    }
}

#[test]
fn large_text_normalization_handles_chunk_boundaries_without_retaining_text() {
    let d = fixture();
    let text = format!("{}\r\n{}🙂\r\n", "a".repeat(65535), "b".repeat(65533));
    fs::write(d.path().join("a.txt"), text.replace("\r\n", "\n")).unwrap();
    git(d.path(), &["add", "a.txt"]);
    git(d.path(), &["commit", "-qm", "large text"]);
    git(d.path(), &["config", "core.autocrlf", "true"]);
    fs::write(d.path().join("a.txt"), &text).unwrap();
    let config = Config::parse_from(["recon", d.path().to_str().unwrap(), "--max-text-bytes", "8"]);
    let engine = Engine::new(&config).unwrap();
    let base = engine
        .repo
        .head_content("a.txt", engine.repo.head().unwrap())
        .unwrap();
    let current = engine.repo.working_content("a.txt").unwrap();
    assert!(base.same(&current));
    assert!(current.text.is_none());
    let mut binary = text.into_bytes();
    binary.extend_from_slice(b"\0\r\n");
    fs::write(d.path().join("a.txt"), &binary).unwrap();
    assert_eq!(
        engine.repo.working_content("a.txt").unwrap().hash,
        *blake3::hash(&binary).as_bytes()
    );
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
    assert!(!m.files.contains_key("a.txt"));
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
fn observed_root_is_discovered_from_a_subdirectory_and_survives_clear() {
    let d = fixture();
    let nested = d.path().join("nested/deep");
    fs::create_dir_all(&nested).unwrap();
    let config = Config::parse_from(["recon", nested.to_str().unwrap()]);
    let mut engine = Engine::new(&config).unwrap();
    let mut model = Model::new(10, 10240);
    for clear in [false, true, false] {
        reset(&mut engine, &mut model, clear);
        assert_eq!(model.root, d.path().canonicalize().unwrap());
    }
}

#[test]
fn restored_deletion_leaves_the_tree_but_preserves_history() {
    let d = fixture();
    let (mut e, mut m) = setup(&d);
    reset(&mut e, &mut m, false);
    fs::remove_file(d.path().join("a.txt")).unwrap();
    refresh(&mut e, &mut m);
    assert_eq!(m.files["a.txt"].status, Status::Deleted);
    assert_eq!(m.entries.len(), 1);
    git(d.path(), &["restore", "a.txt"]);
    refresh(&mut e, &mut m);
    assert!(!m.files.contains_key("a.txt"));
    assert!(m.modification_times.is_empty());
    assert_eq!(m.entries.len(), 2);
    assert!(m.entries[0].diff.contains("-one"));
    assert!(m.entries[1].diff.contains("+one"));
    refresh(&mut e, &mut m);
    assert!(m.files.is_empty());
    assert_eq!(m.entries.len(), 2);
    fs::write(d.path().join("a.txt"), "edited again\n").unwrap();
    refresh(&mut e, &mut m);
    assert_eq!(m.files["a.txt"].status, Status::Modified);
    assert_eq!(m.entries.len(), 3);
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
    assert_eq!(m.root, path.canonicalize().unwrap());
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

#[test]
fn deleted_untracked_files_leave_the_tree_but_keep_history() {
    for clear in [false, true] {
        let d = fixture();
        let path = d.path().join("temporary.txt");
        fs::write(&path, "initial\n").unwrap();
        let (mut e, mut m) = setup(&d);
        reset(&mut e, &mut m, clear);
        fs::write(&path, "edited\n").unwrap();
        refresh(&mut e, &mut m);
        assert!(m.files.contains_key("temporary.txt"));
        fs::remove_file(&path).unwrap();
        refresh(&mut e, &mut m);
        assert!(!m.files.contains_key("temporary.txt"));
        assert!(
            m.entries
                .iter()
                .any(|entry| entry.path == "temporary.txt" && entry.activity == "deleted")
        );
        refresh(&mut e, &mut m);
        assert!(!m.files.contains_key("temporary.txt"));
        fs::write(&path, "recreated\n").unwrap();
        refresh(&mut e, &mut m);
        assert!(m.files.contains_key("temporary.txt"));
        assert!(m.entries.back().unwrap().diff.contains("+recreated"));
    }
}

#[test]
fn recency_ranks_filesystem_mtimes_without_inventing_edit_history() {
    let d = fixture();
    let a = d.path().join("a.txt");
    let b = d.path().join("b.txt");
    let first = std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let second = first + Duration::from_secs(60);
    let newest = second + Duration::from_secs(60);
    let stamp = |path: &Path, time| {
        fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(time)
            .unwrap()
    };
    fs::write(&a, "a changed\n").unwrap();
    stamp(&a, first);
    fs::write(&b, "b\n").unwrap();
    stamp(&b, second);
    let (mut e, mut m) = setup(&d);
    reset(&mut e, &mut m, false);
    let age = |m: &Model, path: &str| {
        let times: Vec<_> = m.modification_times.keys().copied().collect();
        recon::animation::relative_age(m.files[path].modified_at, &times)
    };
    assert_eq!(age(&m, "a.txt"), 1.0);
    assert_eq!(age(&m, "b.txt"), 0.0);
    let activity = m.files["a.txt"].activity;
    let entries = m.entries.len();
    stamp(&a, newest);
    refresh(&mut e, &mut m); // Metadata-only notifications do not invent edits.
    git(d.path(), &["add", "a.txt"]);
    refresh(&mut e, &mut m);
    assert_eq!(m.entries.len(), entries);
    assert_eq!(m.files["a.txt"].activity, activity);
    assert_eq!(age(&m, "a.txt"), 0.0);
    fs::write(&a, "newest\n").unwrap();
    stamp(&a, newest);
    refresh(&mut e, &mut m);
    assert_eq!(age(&m, "a.txt"), 0.0);
    assert_eq!(age(&m, "b.txt"), 1.0);
    fs::write(&b, "tie\n").unwrap();
    stamp(&b, newest);
    refresh(&mut e, &mut m);
    assert_eq!(m.modification_times.len(), 1);
    assert_eq!(m.modification_times[&newest], 2);
    assert_eq!(age(&m, "a.txt"), age(&m, "b.txt"));
}

#[test]
fn branch_labels_refresh_even_when_switching_to_the_same_commit() {
    let d = fixture();
    let (mut e, mut m) = setup(&d);
    reset(&mut e, &mut m, false);
    let original = m.branch.clone();
    assert!(!original.contains("unborn"));
    git(d.path(), &["checkout", "-qb", "same-commit"]);
    refresh(&mut e, &mut m);
    assert_eq!(m.branch, "same-commit");
    assert!(m.message.contains("HEAD changed"));
    git(d.path(), &["checkout", "--detach", "-q"]);
    refresh(&mut e, &mut m);
    assert!(m.branch.starts_with("detached "));
    let empty = tempfile::tempdir().unwrap();
    git(empty.path(), &["init", "-q"]);
    let (mut e, mut m) = setup(&empty);
    reset(&mut e, &mut m, false);
    assert!(m.branch.ends_with("(unborn)"));
}
