//! Concurrent KvStore tests.

use cersei_vms::prelude::*;
use std::sync::Arc;

#[tokio::test]
async fn concurrent_writes_serialize_versions() {
    let kv = Arc::new(KvStore::in_memory());
    let mut handles = Vec::new();
    for i in 0..32 {
        let kv = kv.clone();
        handles.push(tokio::spawn(async move {
            kv.set(format!("k{i}"), format!("v{i}").into_bytes())
                .unwrap();
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    assert_eq!(kv.len(), 32);
    for i in 0..32 {
        let entry = kv.get(&format!("k{i}")).unwrap();
        assert_eq!(entry.value, format!("v{i}").into_bytes());
    }
}

#[tokio::test]
async fn cas_blocks_stale_writers() {
    let kv = KvStore::in_memory();
    let v1 = kv.set("k", b"one".to_vec()).unwrap();
    // CAS with stale expected version (None) must fail.
    let stale = kv.cas("k", None, b"two".to_vec()).unwrap();
    assert!(stale.is_none());
    // CAS with the right version succeeds.
    let updated = kv.cas("k", Some(v1.version), b"two".to_vec()).unwrap();
    assert!(updated.is_some());
    assert_eq!(kv.get("k").unwrap().value, b"two".to_vec());
}

#[tokio::test]
async fn journal_survives_reopen() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("kv.json");
    {
        let kv = KvStore::open(&path).unwrap();
        kv.set("a", b"1".to_vec()).unwrap();
        kv.set("b", b"22".to_vec()).unwrap();
    }
    let kv = KvStore::open(&path).unwrap();
    assert_eq!(kv.get("a").unwrap().value, b"1".to_vec());
    assert_eq!(kv.get("b").unwrap().value, b"22".to_vec());
}
