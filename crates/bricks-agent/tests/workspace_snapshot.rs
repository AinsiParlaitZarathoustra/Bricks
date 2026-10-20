//! The parent's snapshot, from a real git repository: a calm tree is
//! captured exactly, the limits leave out what they name (with the reason),
//! odd entries (links, pipes) never block, and a file rewritten during the
//! capture is either captured whole or refused as `Inconsistent`, never
//! copied torn.

#![cfg(unix)]

use bricks_agent::agents::workspace::{SnapshotLimits, WorkspaceManager, WsError};
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) {
    let o = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
}

/// A repository with `a.txt` and `c.txt` committed.
fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    git(d.path(), &["init", "-q", "-b", "main"]);
    std::fs::write(d.path().join("a.txt"), "one\n").unwrap();
    std::fs::write(d.path().join("c.txt"), "gone soon\n").unwrap();
    git(d.path(), &["add", "."]);
    git(d.path(), &["commit", "-q", "-m", "init"]);
    d
}

fn manager(limits: SnapshotLimits) -> (tempfile::TempDir, WorkspaceManager) {
    let base = tempfile::tempdir().unwrap();
    let m = WorkspaceManager::new(Some(base.path().to_path_buf()), limits);
    (base, m)
}

#[tokio::test]
async fn a_calm_tree_is_captured_exactly_and_the_index_is_not_written() {
    let r = repo();
    let d = r.path();
    std::fs::write(d.join("a.txt"), "one\ntwo\n").unwrap();
    std::fs::write(d.join("b.txt"), "staged\n").unwrap();
    git(d, &["add", "b.txt"]);
    std::fs::remove_file(d.join("c.txt")).unwrap();
    std::fs::create_dir_all(d.join("new")).unwrap();
    std::fs::write(d.join("new/nested.txt"), "nested\n").unwrap();
    std::fs::write(d.join(".gitignore"), "*.log\n").unwrap();
    std::fs::write(d.join("build.log"), "ignored\n").unwrap();
    git(d, &["add", ".gitignore"]);
    let index_before = std::fs::read(d.join(".git/index")).unwrap();

    let (_base, m) = manager(SnapshotLimits::default());
    let snap = m.snapshot(d).await.unwrap();

    let expected_patch = Command::new("git")
        .args(["diff", "--binary", "--full-index", "HEAD", "--"])
        .current_dir(d)
        .output()
        .unwrap()
        .stdout;
    assert_eq!(std::fs::read(&snap.patch).unwrap(), expected_patch);
    assert_eq!(snap.patch_bytes as usize, expected_patch.len());
    assert_eq!(snap.untracked, vec!["new/nested.txt".to_string()]);
    assert_eq!(
        std::fs::read_to_string(snap.untracked_dir.join("new/nested.txt")).unwrap(),
        "nested\n"
    );
    assert!(snap.excluded.is_empty(), "{:?}", snap.excluded);
    assert!(
        !snap.untracked.iter().any(|p| p.ends_with(".log")),
        "ignored files are not captured"
    );
    assert_eq!(snap.rel_cwd, PathBuf::new());
    assert_eq!(
        std::fs::read(d.join(".git/index")).unwrap(),
        index_before,
        "the parent's index is never written"
    );
}

#[tokio::test]
async fn a_snapshot_from_a_subdirectory_names_its_place_in_the_repository() {
    let r = repo();
    let sub = r.path().join("pkg/inner");
    std::fs::create_dir_all(&sub).unwrap();
    let (_base, m) = manager(SnapshotLimits::default());
    let snap = m.snapshot(&sub).await.unwrap();
    assert_eq!(snap.rel_cwd, PathBuf::from("pkg/inner"));
    assert_eq!(
        snap.repo_root.canonicalize().unwrap(),
        r.path().canonicalize().unwrap()
    );
}

#[tokio::test]
async fn a_directory_outside_any_repository_is_refused_as_not_a_repo() {
    let d = tempfile::tempdir().unwrap();
    let (_base, m) = manager(SnapshotLimits::default());
    let err = m.snapshot(d.path()).await.unwrap_err();
    assert!(
        matches!(err, WsError::NotARepo(_) | WsError::Git(_)),
        "{err:?}"
    );
}

#[tokio::test]
async fn a_repository_with_submodules_is_refused_not_guessed() {
    let r = repo();
    std::fs::write(r.path().join(".gitmodules"), "[submodule \"x\"]\n").unwrap();
    let (_base, m) = manager(SnapshotLimits::default());
    let err = m.snapshot(r.path()).await.unwrap_err();
    assert!(matches!(err, WsError::Unsupported(_)), "{err:?}");
}

#[tokio::test]
async fn the_file_count_limit_leaves_out_the_rest_with_its_reason() {
    let r = repo();
    for n in ["u1.txt", "u2.txt", "u3.txt"] {
        std::fs::write(r.path().join(n), "x").unwrap();
    }
    let limits = SnapshotLimits {
        max_untracked_files: 2,
        ..SnapshotLimits::default()
    };
    let (_base, m) = manager(limits);
    let snap = m.snapshot(r.path()).await.unwrap();
    assert_eq!(
        snap.untracked,
        vec!["u1.txt".to_string(), "u2.txt".to_string()]
    );
    assert_eq!(
        snap.excluded,
        vec!["u3.txt: more than 2 untracked files".to_string()]
    );
}

#[tokio::test]
async fn the_per_file_limit_leaves_out_a_large_file_only() {
    let r = repo();
    std::fs::write(r.path().join("big.bin"), vec![b'x'; 2_000]).unwrap();
    std::fs::write(r.path().join("small.txt"), "ok").unwrap();
    let limits = SnapshotLimits {
        max_untracked_file_bytes: 1_000,
        ..SnapshotLimits::default()
    };
    let (_base, m) = manager(limits);
    let snap = m.snapshot(r.path()).await.unwrap();
    assert_eq!(snap.untracked, vec!["small.txt".to_string()]);
    assert_eq!(
        snap.excluded,
        vec!["big.bin: 2000 bytes > 1000".to_string()]
    );
}

#[tokio::test]
async fn the_total_limit_counts_the_patch_and_stops_the_copies() {
    let r = repo();
    std::fs::write(r.path().join("a.txt"), "changed\n").unwrap();
    std::fs::write(r.path().join("p.txt"), vec![b'p'; 400]).unwrap();
    std::fs::write(r.path().join("q.txt"), vec![b'q'; 400]).unwrap();
    let limits = SnapshotLimits {
        max_total_bytes: 600,
        ..SnapshotLimits::default()
    };
    let (_base, m) = manager(limits);
    let snap = m.snapshot(r.path()).await.unwrap();
    // The tracked patch (about 90 bytes) counts first: p.txt fits, q.txt does not.
    assert_eq!(snap.untracked, vec!["p.txt".to_string()]);
    assert_eq!(
        snap.excluded,
        vec!["q.txt: snapshot limit of 600 bytes reached".to_string()]
    );
}

#[tokio::test]
async fn links_and_pipes_are_left_out_and_a_pipe_never_blocks_the_capture() {
    let r = repo();
    std::os::unix::fs::symlink("a.txt", r.path().join("link")).unwrap();
    let status = Command::new("mkfifo")
        .arg(r.path().join("pipe"))
        .status()
        .unwrap();
    assert!(status.success());
    let (_base, m) = manager(SnapshotLimits::default());
    let started = std::time::Instant::now();
    let snap = tokio::time::timeout(std::time::Duration::from_secs(20), m.snapshot(r.path()))
        .await
        .expect("a pipe does not block the capture")
        .unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(20));
    assert!(snap.untracked.is_empty(), "{:?}", snap.untracked);
    // git lists the symlink as untracked; it does not list a pipe at all.
    assert_eq!(snap.excluded, vec!["link: not a regular file".to_string()]);
}

/// A writer rewrites `busy.txt` between two full contents of the same
/// length, creates and removes `n*.txt` files, all while snapshots run. Every
/// accepted snapshot holds whole files only (each copy equals one of the two
/// contents), and every listed file exists in the snapshot. A refusal is
/// `Inconsistent`, never anything else. Once the writer stops, captures pass.
#[tokio::test(flavor = "multi_thread", worker_threads = 3)]
async fn a_file_rewritten_during_the_capture_is_captured_whole_or_refused() {
    let r = repo();
    let root = r.path().to_path_buf();
    let a = vec![b'a'; 8_192];
    let b = vec![b'b'; 8_192];
    std::fs::write(root.join("busy.txt"), &a).unwrap();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let writer = {
        let (root, stop, a, b) = (root.clone(), stop.clone(), a.clone(), b.clone());
        std::thread::spawn(move || {
            let mut flip = false;
            let mut n = 0u32;
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                flip = !flip;
                std::fs::write(root.join("busy.txt"), if flip { &b } else { &a }).unwrap();
                let extra = root.join(format!("n{}.txt", n % 4));
                if n.is_multiple_of(2) {
                    std::fs::write(&extra, "tmp").unwrap();
                } else {
                    let _ = std::fs::remove_file(&extra);
                }
                n += 1;
                std::thread::sleep(std::time::Duration::from_millis(3));
            }
        })
    };
    let (_base, m) = manager(SnapshotLimits::default());
    let (mut ok, mut refused) = (0, 0);
    for _ in 0..15 {
        match m.snapshot(&root).await {
            Ok(snap) => {
                ok += 1;
                if let Ok(copy) = std::fs::read(snap.untracked_dir.join("busy.txt")) {
                    assert!(
                        copy == a || copy == b,
                        "a torn copy of busy.txt ({} bytes)",
                        copy.len()
                    );
                }
                for rel in &snap.untracked {
                    assert!(
                        snap.untracked_dir.join(rel).is_file(),
                        "{rel} is listed but not copied"
                    );
                }
            }
            Err(WsError::Inconsistent(_)) => refused += 1,
            Err(e) => panic!("only a refusal is allowed, got {e:?}"),
        }
    }
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    writer.join().unwrap();
    assert_eq!(ok + refused, 15);
    eprintln!("captures during writes: {ok} accepted, {refused} refused as inconsistent");

    // Quiet tree: every capture passes, and carries the last content.
    let last = std::fs::read(root.join("busy.txt")).unwrap();
    for _ in 0..5 {
        let snap = m.snapshot(&root).await.expect("a quiet tree is captured");
        assert_eq!(
            std::fs::read(snap.untracked_dir.join("busy.txt")).unwrap(),
            last
        );
    }
}

/// Every untracked file left out says why. The listing is cut at four times
/// the file limit, and what lies beyond the cut must still be reported, not
/// dropped without a word.
#[tokio::test]
async fn every_untracked_file_left_out_is_reported_with_its_reason() {
    let r = repo();
    for i in 1..=10 {
        std::fs::write(r.path().join(format!("u{i:02}.txt")), "x").unwrap();
    }
    let limits = SnapshotLimits {
        max_untracked_files: 2,
        ..SnapshotLimits::default()
    };
    let (_base, m) = manager(limits);
    let snap = m.snapshot(r.path()).await.unwrap();
    assert_eq!(snap.untracked.len(), 2);
    assert_eq!(
        snap.excluded.len(),
        8,
        "one reason per file left out: {:?}",
        snap.excluded
    );
}
