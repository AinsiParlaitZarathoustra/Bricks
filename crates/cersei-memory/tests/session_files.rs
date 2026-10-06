//! A session's internal files (raw history, compaction snapshots, saved
//! outputs) belong to it: they are not listed as sessions and are deleted
//! with it.

use cersei_memory::{session_keys, JsonlMemory, Memory};
use cersei_types::Message;

async fn put(m: &JsonlMemory, key: &str) {
    m.store(key, &[Message::user("x")]).await.unwrap();
}

#[tokio::test]
async fn only_real_sessions_are_listed() {
    let dir = tempfile::tempdir().unwrap();
    let m = JsonlMemory::new(dir.path());
    for key in [
        "a",
        "a.raw",
        "a.compaction-1",
        "a.compaction-12",
        // Real sessions that merely look like internal keys: no base exists.
        "notes.raw",
        "orphan.compaction-3",
        // Not the convention at all.
        "v1.2",
        "a.compaction-x",
        "a.compaction-",
    ] {
        put(&m, key).await;
    }
    std::fs::create_dir_all(m.session_files_dir("a").unwrap()).unwrap();

    let mut ids: Vec<String> = m
        .sessions()
        .await
        .unwrap()
        .into_iter()
        .map(|s| s.id)
        .collect();
    ids.sort();
    assert_eq!(
        ids,
        [
            "a",
            "a.compaction-",
            "a.compaction-x",
            "notes.raw",
            "orphan.compaction-3",
            "v1.2"
        ]
    );
}

#[tokio::test]
async fn deleting_a_session_removes_its_internal_files_only() {
    let dir = tempfile::tempdir().unwrap();
    let m = JsonlMemory::new(dir.path());
    for key in [
        "a",
        "a.raw",
        "a.compaction-1",
        "a.compaction-2",
        "ab",
        "ab.raw",
        "b",
    ] {
        put(&m, key).await;
    }
    let files = m.session_files_dir("a").unwrap();
    std::fs::create_dir_all(&files).unwrap();
    std::fs::write(files.join("00001-Bash.txt"), "out").unwrap();

    m.delete("a").await.unwrap();

    let mut left: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    left.sort();
    assert_eq!(left, ["ab.jsonl", "ab.raw.jsonl", "b.jsonl"]);
}

#[test]
fn key_conventions_round_trip() {
    assert_eq!(
        session_keys::parent(&session_keys::raw_history("s.1")),
        Some("s.1")
    );
    assert_eq!(
        session_keys::parent(&session_keys::snapshot("s", 7)),
        Some("s")
    );
    assert_eq!(session_keys::parent("s"), None);
    assert_eq!(session_keys::parent(".raw"), None);
}
