//! Isolated workspaces on real git repositories: the parent's dirty state,
//! ChangeSets that exclude inherited changes, apply, conflicts, refusals.

use cersei_agent::agents::workspace::{ChangeSetState, WorkspaceManager, WsError};
use std::path::Path;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> String {
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
    String::from_utf8_lossy(&o.stdout).to_string()
}

/// A repository with one commit: a.txt, b.txt, gone.txt.
fn repo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    git(d.path(), &["init", "-q", "-b", "main"]);
    std::fs::write(d.path().join("a.txt"), "one\ntwo\nthree\n").unwrap();
    std::fs::write(d.path().join("b.txt"), "bee\n").unwrap();
    std::fs::write(d.path().join("gone.txt"), "will be deleted\n").unwrap();
    std::fs::write(d.path().join(".gitignore"), "target/\n").unwrap();
    git(d.path(), &["add", "."]);
    git(d.path(), &["commit", "-q", "-m", "init"]);
    d
}

fn manager() -> (WorkspaceManager, tempfile::TempDir) {
    let base = tempfile::tempdir().unwrap();
    (
        WorkspaceManager::new(Some(base.path().to_path_buf()), Default::default()),
        base,
    )
}

fn index_hash(repo: &Path) -> Vec<u8> {
    std::fs::read(repo.join(".git/index")).unwrap()
}

#[tokio::test]
async fn children_start_from_the_parents_dirty_state_without_touching_it() {
    let r = repo();
    let p = r.path();
    // Staged, unstaged, new, deleted, ignored.
    std::fs::write(p.join("a.txt"), "one\ntwo staged\nthree\n").unwrap();
    git(p, &["add", "a.txt"]);
    std::fs::write(p.join("b.txt"), "bee unstaged\n").unwrap();
    std::fs::write(p.join("new.txt"), "untracked\n").unwrap();
    std::fs::remove_file(p.join("gone.txt")).unwrap();
    std::fs::create_dir_all(p.join("target")).unwrap();
    std::fs::write(p.join("target/cache.bin"), "ignored").unwrap();
    let status_before = git(p, &["status", "--porcelain"]);
    let index_before = index_hash(p);

    let (m, _b) = manager();
    let snap = m.snapshot(p).await.unwrap();
    assert!(snap.untracked.contains(&"new.txt".to_string()));
    assert!(
        !snap.untracked.iter().any(|u| u.starts_with("target")),
        "ignored files are not copied"
    );

    // The parent is untouched: same status, same index bytes.
    assert_eq!(git(p, &["status", "--porcelain"]), status_before);
    assert_eq!(index_hash(p), index_before);

    // Two children, the same starting point.
    let w1 = m.create(&snap, "run_t", "agent_one").await.unwrap();
    let w2 = m.create(&snap, "run_t", "agent_two").await.unwrap();
    for w in [&w1, &w2] {
        assert_eq!(
            std::fs::read_to_string(w.root.join("a.txt")).unwrap(),
            "one\ntwo staged\nthree\n"
        );
        assert_eq!(
            std::fs::read_to_string(w.root.join("b.txt")).unwrap(),
            "bee unstaged\n"
        );
        assert_eq!(
            std::fs::read_to_string(w.root.join("new.txt")).unwrap(),
            "untracked\n"
        );
        assert!(!w.root.join("gone.txt").exists());
        assert!(!w.root.join("target").exists());
        assert!(w.branch.starts_with("bricks/run_t/agent_"));
    }
    assert_eq!(w1.baseline_tree, w2.baseline_tree);
    // No commit was made anywhere: the branches point at the base.
    assert_eq!(git(p, &["rev-parse", &w1.branch]).trim(), snap.base_sha);
    assert_eq!(git(p, &["rev-list", "--count", "--all"]).trim(), "1");

    // The child changes one file: its ChangeSet has that file only, not the
    // inherited changes.
    std::fs::write(
        w1.root.join("a.txt"),
        "one\ntwo staged\nthree\nfour by child\n",
    )
    .unwrap();
    std::fs::write(w1.root.join("child.txt"), "made by the child\n").unwrap();
    let cs = m
        .finalize("agent_one", "task", vec![])
        .await
        .unwrap()
        .unwrap();
    let mut files: Vec<&str> = cs.files.iter().map(|f| f.path.as_str()).collect();
    files.sort();
    assert_eq!(files, vec!["a.txt", "child.txt"]);
    let patch = std::fs::read_to_string(&cs.patch).unwrap();
    assert!(patch.contains("+four by child"));
    assert!(!patch.contains("bee unstaged") && !patch.contains("untracked"));
    // An unchanged child leaves nothing behind.
    assert!(m
        .finalize("agent_two", "task", vec![])
        .await
        .unwrap()
        .is_none());
    assert!(!w2.root.exists());
    assert!(git(p, &["branch", "--list", &w2.branch]).trim().is_empty());

    // Apply: only the child's change lands in the parent's tree.
    let done = m.apply(&cs.id, p).await.unwrap();
    assert_eq!(done.state, ChangeSetState::Applied);
    assert_eq!(
        std::fs::read_to_string(p.join("a.txt")).unwrap(),
        "one\ntwo staged\nthree\nfour by child\n"
    );
    assert_eq!(
        std::fs::read_to_string(p.join("child.txt")).unwrap(),
        "made by the child\n"
    );
    assert_eq!(
        std::fs::read_to_string(p.join("b.txt")).unwrap(),
        "bee unstaged\n"
    );
    assert_eq!(
        git(p, &["rev-list", "--count", "--all"]).trim(),
        "1",
        "nothing committed"
    );
    assert!(!w1.root.exists(), "an applied worktree is removed");
}

#[tokio::test]
async fn concurrent_edits_of_the_same_content_conflict_without_overwriting() {
    let r = repo();
    let p = r.path();
    let (m, _b) = manager();
    let snap = m.snapshot(p).await.unwrap();
    let w1 = m.create(&snap, "run_c", "agent_a").await.unwrap();
    let w2 = m.create(&snap, "run_c", "agent_b").await.unwrap();
    std::fs::write(w1.root.join("a.txt"), "one\nTWO by a\nthree\n").unwrap();
    std::fs::write(w2.root.join("a.txt"), "one\nTWO by b\nthree\n").unwrap();
    let c1 = m.finalize("agent_a", "t", vec![]).await.unwrap().unwrap();
    let c2 = m.finalize("agent_b", "t", vec![]).await.unwrap().unwrap();
    assert_ne!(w1.branch, w2.branch);
    m.apply(&c1.id, p).await.unwrap();
    let e = m.apply(&c2.id, p).await.unwrap_err();
    assert!(matches!(e, WsError::Conflict { .. }), "{e}");
    assert!(e.to_string().contains("WorktreeConflict"));
    // Nothing overwritten; both versions kept.
    assert_eq!(
        std::fs::read_to_string(p.join("a.txt")).unwrap(),
        "one\nTWO by a\nthree\n"
    );
    assert_eq!(m.changeset(&c2.id).unwrap().state, ChangeSetState::Conflict);
    assert!(
        w2.root.join("a.txt").exists(),
        "the conflicting work is kept"
    );
    // Discard: the worktree goes, the patch stays as a record.
    let d = m.discard(&c2.id).await.unwrap();
    assert_eq!(d.state, ChangeSetState::Discarded);
    assert!(!w2.root.exists());
    assert!(Path::new(&d.patch).exists());
}

#[tokio::test]
async fn an_external_change_of_the_parent_after_the_snapshot_is_a_conflict() {
    let r = repo();
    let p = r.path();
    let (m, _b) = manager();
    let snap = m.snapshot(p).await.unwrap();
    let w = m.create(&snap, "run_e", "agent_e").await.unwrap();
    std::fs::write(w.root.join("b.txt"), "bee by child\n").unwrap();
    let cs = m.finalize("agent_e", "t", vec![]).await.unwrap().unwrap();
    // Meanwhile, someone edits the same line in the parent.
    std::fs::write(p.join("b.txt"), "bee by someone else\n").unwrap();
    assert!(matches!(
        m.apply(&cs.id, p).await,
        Err(WsError::Conflict { .. })
    ));
    assert_eq!(
        std::fs::read_to_string(p.join("b.txt")).unwrap(),
        "bee by someone else\n"
    );
}

#[tokio::test]
async fn a_grandchild_starts_from_its_parents_modified_worktree() {
    let r = repo();
    let p = r.path();
    std::fs::write(p.join("b.txt"), "parent edit\n").unwrap();
    let (m, _b) = manager();
    let snap = m.snapshot(p).await.unwrap();
    let child = m.create(&snap, "run_g", "agent_child").await.unwrap();
    // The child modifies its worktree, then delegates.
    std::fs::write(child.root.join("a.txt"), "child edit\n").unwrap();
    let snap2 = m.snapshot(&child.root).await.unwrap();
    let gc = m.create(&snap2, "run_g", "agent_grand").await.unwrap();
    assert_eq!(
        std::fs::read_to_string(gc.root.join("b.txt")).unwrap(),
        "parent edit\n"
    );
    assert_eq!(
        std::fs::read_to_string(gc.root.join("a.txt")).unwrap(),
        "child edit\n"
    );
    std::fs::write(gc.root.join("grand.txt"), "x\n").unwrap();
    let cs = m
        .finalize("agent_grand", "t", vec![])
        .await
        .unwrap()
        .unwrap();
    let files: Vec<&str> = cs.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(
        files,
        vec!["grand.txt"],
        "inherited edits are not the grandchild's"
    );
}

#[tokio::test]
async fn unsupported_repositories_are_refused_before_anything_starts() {
    let (m, _b) = manager();
    // Not a repository.
    let plain = tempfile::tempdir().unwrap();
    assert!(matches!(
        m.snapshot(plain.path()).await,
        Err(WsError::NotARepo(_))
    ));
    // No HEAD.
    let empty = tempfile::tempdir().unwrap();
    git(empty.path(), &["init", "-q"]);
    assert_eq!(m.snapshot(empty.path()).await.unwrap_err(), WsError::NoHead);
    // Submodules.
    let r = repo();
    std::fs::write(r.path().join(".gitmodules"), "[submodule \"x\"]\n").unwrap();
    assert!(matches!(
        m.snapshot(r.path()).await,
        Err(WsError::Unsupported(_))
    ));
    // A branch that already exists is never overwritten.
    let r = repo();
    git(r.path(), &["branch", "bricks/run_x/agent_dup"]);
    let snap = m.snapshot(r.path()).await.unwrap();
    assert!(matches!(
        m.create(&snap, "run_x", "agent_dup").await,
        Err(WsError::Collision(_))
    ));
    // Without a session folder, isolation is unavailable (no fallback).
    let none = WorkspaceManager::new(None, Default::default());
    assert!(matches!(
        none.snapshot(r.path()).await,
        Err(WsError::Unsupported(_))
    ));
}

#[tokio::test]
async fn binary_files_deletions_and_modes_survive_the_round_trip() {
    let r = repo();
    let p = r.path();
    let (m, _b) = manager();
    let snap = m.snapshot(p).await.unwrap();
    let w = m.create(&snap, "run_b", "agent_bin").await.unwrap();
    std::fs::write(w.root.join("img.bin"), [0u8, 159, 146, 150, 0, 1, 2]).unwrap();
    std::fs::remove_file(w.root.join("gone.txt")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(w.root.join("run.sh"), "#!/bin/sh\necho hi\n").unwrap();
        std::fs::set_permissions(
            w.root.join("run.sh"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    let cs = m.finalize("agent_bin", "t", vec![]).await.unwrap().unwrap();
    assert!(cs.files.iter().any(|f| f.path == "img.bin" && f.binary));
    assert!(cs
        .files
        .iter()
        .any(|f| f.path == "gone.txt" && f.status == "D"));
    m.apply(&cs.id, p).await.unwrap();
    assert_eq!(
        std::fs::read(p.join("img.bin")).unwrap(),
        vec![0u8, 159, 146, 150, 0, 1, 2]
    );
    assert!(!p.join("gone.txt").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(p.join("run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert!(mode & 0o111 != 0, "executable bit kept");
    }
}
