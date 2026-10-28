//! Upgrade a real 0.5.44 file, never a user's database.
#![cfg(feature = "structured")]

use bricks_embeddings::HashingEmbeddings;
use bricks_memory::structured::{MemoryConfig, RecallQuery, StructuredMemory};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

const OLD_STORE: &[u8] = include_bytes!("fixtures/grafeo-0.5.44.grafeo");

#[tokio::test]
async fn a_0_5_44_file_migrates_keeps_its_backup_and_reopens_without_duplicates() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("memory.grafeo");
    let backup = dir.path().join("memory.grafeo.pre-0.6");
    std::fs::write(&path, OLD_STORE).unwrap();
    let open = || {
        StructuredMemory::builder(Arc::new(HashingEmbeddings::new(256)))
            .path(&path)
            .config(MemoryConfig::default().with_space("space:legacy"))
            .open()
            .unwrap()
    };
    {
        let memory = open();
        assert_eq!(std::fs::read(&backup).unwrap(), OLD_STORE);
        assert_eq!(memory.stats().episodes, 2);
        let episode = memory.episode("ep_legacy_old_axum").unwrap().unwrap();
        assert_eq!(episode.content, "Bricks utilise Axum et garde ses sources.");
        assert_eq!(
            episode.source_ref.as_deref(),
            Some("legacy-memory:old_axum")
        );
        assert_eq!(episode.occurred_at, Some(1_767_225_600_000));
        memory.process(&CancellationToken::new()).await.unwrap();
        assert_eq!(memory.stats().indexed, 2);
        let recall = memory.recall(&RecallQuery::new("Axum")).await.unwrap();
        assert!(recall.rendered.contains("Axum"));
        memory.close().unwrap();
    }
    {
        let memory = open();
        assert_eq!(memory.stats().episodes, 2);
        assert_eq!(memory.stats().indexed, 2);
        assert!(
            memory
                .recall(&RecallQuery::new("Rust"))
                .await
                .unwrap()
                .rendered
                .contains("Rust")
        );
        memory.close().unwrap();
    }
    // The original graph remains, including its relation and numeric property.
    let db = grafeo::GrafeoDB::open(&path).unwrap();
    let rows = db.execute("MATCH (a:Memory {id: 'old_axum'})-[r:RELATES_TO]->(b:Memory) RETURN a.confidence, r.relationship, b.id").unwrap();
    let row = rows.iter().next().unwrap();
    assert!(matches!(row.first(), Some(grafeo::Value::Float64(v)) if *v == 0.75));
    assert_eq!(row.get(1), Some(&grafeo::Value::from("implementation")));
    assert_eq!(row.get(2), Some(&grafeo::Value::from("old_rust")));
    db.close().unwrap();
    assert_eq!(std::fs::read(backup).unwrap(), OLD_STORE);
}
