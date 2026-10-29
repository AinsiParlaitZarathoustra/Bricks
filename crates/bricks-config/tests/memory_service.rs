//! Acceptance: the desktop's memory stores (`MemoryService`), under a
//! temporary Bricks home. Two scopes: the global memory, and one memory per
//! project id. Isolation, provenance, no project, invalid ids, a store that
//! cannot open, the former single store, reading without creating, clearing
//! refused while a run uses a store, retired projects. The user's
//! `~/.bricks` is never read or written.

use bricks_agent::control::RunMemory;
use bricks_config::{MemoryService, RetiredStore};
use bricks_memory::structured::{EntryCounts, EntryFilter, MemoryError};
use bricks_memory::{LongTermMemory, MemoryScope, MemoryTurn};
use std::fs;
use std::sync::Arc;

fn mem(run: &RunMemory) -> Arc<dyn LongTermMemory> {
    run.memory.clone().expect("the memory is on for this run")
}

/// The message of an error, whatever the type of the success.
fn err_text<T, E: std::fmt::Display>(r: Result<T, E>) -> String {
    match r {
        Ok(_) => panic!("expected an error"),
        Err(e) => e.to_string(),
    }
}

fn turn(role: &str, content: &str) -> MemoryTurn {
    MemoryTurn {
        role: role.into(),
        content: content.into(),
        at: None,
    }
}

// ─── Layout ──────────────────────────────────────────────────────────────────

#[test]
fn the_stores_live_under_the_home_memory_folder() {
    let home = tempfile::tempdir().unwrap();
    let svc = MemoryService::in_home(home.path());
    assert_eq!(svc.global_path(), home.path().join("memory/global.grafeo"));
    assert_eq!(
        svc.project_path("pa"),
        home.path().join("memory/projects/pa.grafeo")
    );
}

#[test]
fn reading_a_memory_never_written_creates_nothing() {
    let home = tempfile::tempdir().unwrap();
    let svc = MemoryService::in_home(home.path());
    assert_eq!(
        svc.counts(MemoryScope::Global, None).unwrap(),
        EntryCounts::default()
    );
    assert_eq!(
        svc.counts(MemoryScope::Project, Some("pa")).unwrap(),
        EntryCounts::default()
    );
    let page = svc
        .entries(MemoryScope::Project, Some("pa"), EntryFilter::All, "", 0, 50)
        .unwrap();
    assert_eq!(page.total, 0);
    assert!(!svc.global_path().exists());
    assert!(!svc.project_path("pa").exists());
    assert!(svc.retired(&[]).is_empty());
    // Same page validation as a store's: 1 to 200 entries.
    assert!(svc.entries(MemoryScope::Global, None, EntryFilter::All, "", 0, 0).is_err());
    assert!(svc.entries(MemoryScope::Global, None, EntryFilter::All, "", 0, 201).is_err());
}

#[tokio::test]
async fn an_invalid_project_id_is_refused_and_nothing_is_created_outside_the_folder() {
    let home = tempfile::tempdir().unwrap();
    let svc = MemoryService::in_home(home.path());
    assert!(svc.counts(MemoryScope::Project, Some("../escape")).is_err());
    assert!(svc.delete_retired("../escape").await.is_err());
    let run = svc.for_run(Some(("../escape", "x")));
    let e = err_text(mem(&run).search(MemoryScope::Project, "x", 5).await);
    assert!(e.contains("invalid project id"), "{e}");
    assert!(!home.path().join("escape.grafeo").exists());
    assert!(!home.path().join("memory/escape.grafeo").exists());
}

// ─── Scopes ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn projects_are_isolated_and_a_global_note_keeps_the_project_that_wrote_it() {
    let home = tempfile::tempdir().unwrap();
    let svc = MemoryService::in_home(home.path());
    let (ra, rb) = (
        svc.for_run(Some(("pa", "Alpha"))),
        svc.for_run(Some(("pb", "Beta"))),
    );
    mem(&ra)
        .write_note(MemoryScope::Project, Some("s1"), "alpha uses postgres")
        .await
        .unwrap();
    mem(&ra)
        .write_note(MemoryScope::Global, Some("s1"), "prefers short answers")
        .await
        .unwrap();

    assert!(
        mem(&rb)
            .search(MemoryScope::Project, "postgres", 5)
            .await
            .unwrap()
            .is_none(),
        "the other project does not see it"
    );
    let found = mem(&rb)
        .search(MemoryScope::Global, "short answers", 5)
        .await
        .unwrap()
        .expect("the global note is shared");
    assert_eq!(found.items, 1);
    let page = svc
        .entries(MemoryScope::Global, None, EntryFilter::Notes, "", 0, 50)
        .unwrap();
    assert_eq!(page.entries[0].project_id.as_deref(), Some("pa"));
    assert_eq!(page.entries[0].session_id.as_deref(), Some("s1"));
}

#[tokio::test]
async fn without_a_project_nothing_is_captured_and_nothing_falls_back_to_global() {
    let home = tempfile::tempdir().unwrap();
    let svc = MemoryService::in_home(home.path());
    let run = svc.for_run(None);
    let m = mem(&run);
    m.record_turns(Some("s"), &[turn("user", "hello there")])
        .await
        .unwrap();
    assert_eq!(svc.counts(MemoryScope::Global, None).unwrap().exchanges, 0);

    let e = err_text(m.search(MemoryScope::Project, "hello", 5).await);
    assert!(e.contains("no project"), "{e}");
    let e = err_text(m.write_note(MemoryScope::Project, Some("s"), "x").await);
    assert!(e.contains("no project"), "{e}");
    assert!(svc.counts(MemoryScope::Project, None).is_err());
    assert_eq!(svc.counts(MemoryScope::Global, None).unwrap().notes, 0);
}

#[tokio::test]
async fn a_project_keeps_its_memory_when_it_moves_to_another_folder() {
    let home = tempfile::tempdir().unwrap();
    let svc = MemoryService::in_home(home.path());
    {
        // The project in its first folder: the run and its note.
        let run = svc.for_run(Some(("pa", "Alpha")));
        mem(&run)
            .write_note(MemoryScope::Project, Some("s1"), "alpha uses postgres")
            .await
            .unwrap();
    }
    // Moved: same id, another name, and a fresh service as after a restart.
    let moved = MemoryService::in_home(home.path()).for_run(Some(("pa", "Alpha moved")));
    let found = mem(&moved)
        .search(MemoryScope::Project, "postgres", 5)
        .await
        .unwrap();
    assert!(found.is_some(), "the memory follows the id");
    assert_eq!(svc.known_name("pa").as_deref(), Some("Alpha moved"));
}

#[tokio::test]
async fn a_project_store_that_cannot_open_leaves_the_global_memory_working() {
    let home = tempfile::tempdir().unwrap();
    // The projects folder is a file: the project store cannot be created.
    fs::create_dir_all(home.path().join("memory")).unwrap();
    fs::write(home.path().join("memory/projects"), b"not a folder").unwrap();
    let svc = MemoryService::in_home(home.path());

    let run = svc.for_run(Some(("pa", "Alpha")));
    let m = mem(&run);
    m.write_note(MemoryScope::Global, Some("s1"), "prefers short answers")
        .await
        .unwrap();
    let ctx = m
        .recall_context("short answers", 1000)
        .await
        .unwrap()
        .expect("the global answer is kept");
    assert!(ctx.items >= 1, "{ctx:?}");
    assert!(
        ctx.warnings
            .iter()
            .any(|w| w.contains("project memory is unavailable")),
        "{:?}",
        ctx.warnings
    );
    let e = err_text(m.write_note(MemoryScope::Project, Some("s1"), "x").await);
    assert!(e.contains("unavailable"), "{e}");
}

#[tokio::test]
async fn the_former_single_store_is_left_as_it_is_and_never_migrated() {
    let home = tempfile::tempdir().unwrap();
    fs::create_dir_all(home.path().join("memory")).unwrap();
    fs::write(home.path().join("memory/memory.grafeo"), b"legacy bytes").unwrap();
    let svc = MemoryService::in_home(home.path());
    assert!(svc.legacy_store_exists());

    let run = svc.for_run(Some(("pa", "Alpha")));
    mem(&run)
        .write_note(MemoryScope::Global, Some("s1"), "a new global note")
        .await
        .unwrap();
    assert_eq!(
        svc.entries(MemoryScope::Global, None, EntryFilter::All, "", 0, 50)
            .unwrap()
            .total,
        1,
        "only the new note: nothing migrated"
    );
    assert_eq!(
        fs::read(home.path().join("memory/memory.grafeo")).unwrap(),
        b"legacy bytes"
    );
}

// ─── Clearing ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn clearing_is_refused_while_a_run_uses_the_store_and_only_that_scope_is_cleared() {
    let home = tempfile::tempdir().unwrap();
    let svc = MemoryService::in_home(home.path());
    let held = svc.for_run(Some(("pa", "Alpha")));
    mem(&held)
        .write_note(MemoryScope::Project, Some("s1"), "alpha note")
        .await
        .unwrap();
    mem(&held)
        .write_note(MemoryScope::Global, Some("s1"), "global note")
        .await
        .unwrap();

    assert!(matches!(
        svc.delete(MemoryScope::Project, Some("pa")).await,
        Err(MemoryError::InUse(_))
    ));
    assert!(matches!(
        svc.delete(MemoryScope::Global, None).await,
        Err(MemoryError::InUse(_))
    ));
    drop(held);

    let report = svc.delete(MemoryScope::Project, Some("pa")).await.unwrap();
    assert_eq!(report.episodes, 1);
    assert_eq!(svc.counts(MemoryScope::Project, Some("pa")).unwrap().notes, 0);
    assert_eq!(
        svc.counts(MemoryScope::Global, None).unwrap().notes,
        1,
        "the global memory is untouched by a project deletion"
    );
    svc.delete(MemoryScope::Global, None).await.unwrap();
    assert_eq!(svc.counts(MemoryScope::Global, None).unwrap().notes, 0);
}

// ─── Retired projects ────────────────────────────────────────────────────────

#[tokio::test]
async fn a_retired_project_store_is_listed_with_its_name_and_refused_while_in_use() {
    let home = tempfile::tempdir().unwrap();
    let svc = MemoryService::in_home(home.path());
    for (id, name) in [("pa", "Alpha"), ("pb", "Beta")] {
        let run = svc.for_run(Some((id, name)));
        mem(&run)
            .write_note(MemoryScope::Project, Some("s1"), &format!("note of {id}"))
            .await
            .unwrap();
    }
    assert_eq!(
        svc.retired(&["pb".into()]),
        vec![RetiredStore {
            project_id: "pa".into(),
            name: Some("Alpha".into()),
        }]
    );

    let held = svc.for_run(Some(("pa", "Alpha")));
    assert!(matches!(
        svc.delete_retired("pa").await,
        Err(MemoryError::InUse(_))
    ));
    drop(held);
    svc.delete_retired("pa").await.unwrap();
    assert_eq!(svc.counts(MemoryScope::Project, Some("pb")).unwrap().notes, 1);
}

#[tokio::test]
async fn a_retired_project_memory_is_gone_from_the_list_once_deleted() {
    // Regression guard: a deleted retired memory must not stay listed.
    let home = tempfile::tempdir().unwrap();
    let svc = MemoryService::in_home(home.path());
    {
        let run = svc.for_run(Some(("pa", "Alpha")));
        mem(&run)
            .write_note(MemoryScope::Project, Some("s1"), "note of pa")
            .await
            .unwrap();
    }
    svc.delete_retired("pa").await.unwrap();
    assert!(
        svc.retired(&[]).is_empty(),
        "listed again after deletion: {:?}",
        svc.retired(&[])
    );
}

#[tokio::test]
async fn a_retired_store_that_is_closed_is_removed_from_disk() {
    let home = tempfile::tempdir().unwrap();
    let projects = home.path().join("memory/projects");
    fs::create_dir_all(&projects).unwrap();
    fs::write(projects.join("pz.grafeo"), b"not a store").unwrap();
    fs::write(projects.join("pz.json"), r#"{"name":"Zeta"}"#).unwrap();
    fs::write(projects.join("bad id.grafeo"), b"ignored").unwrap();
    let svc = MemoryService::in_home(home.path());

    assert_eq!(
        svc.retired(&[]),
        vec![RetiredStore {
            project_id: "pz".into(),
            name: Some("Zeta".into()),
        }],
        "an invalid id is not listed"
    );
    svc.delete_retired("pz").await.unwrap();
    assert!(!projects.join("pz.grafeo").exists());
    assert!(!projects.join("pz.json").exists());
    assert!(svc.retired(&[]).is_empty());
    assert!(svc.delete_retired("../pz").await.is_err());
}
