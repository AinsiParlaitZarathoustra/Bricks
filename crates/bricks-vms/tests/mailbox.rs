//! Cross-sandbox mailbox tests.

use bricks_vms::prelude::*;
use serde_json::json;

#[tokio::test]
async fn two_sandboxes_exchange_messages() {
    let mailbox = Mailbox::new();
    let topic = "test/chat";
    let mut sub_a = mailbox.subscribe(topic);
    let mut sub_b = mailbox.subscribe(topic);

    let from = SandboxId::new();
    let from2 = from.clone();
    for i in 0..3u32 {
        mailbox
            .publish(topic, from2.clone(), json!({ "i": i }))
            .unwrap();
    }

    for expected in 0..3u32 {
        let a = sub_a.recv().await.unwrap();
        let b = sub_b.recv().await.unwrap();
        assert_eq!(a.payload["i"].as_u64(), Some(expected as u64));
        assert_eq!(b.payload["i"].as_u64(), Some(expected as u64));
    }
}

#[tokio::test]
async fn topics_are_isolated() {
    let mailbox = Mailbox::new();
    let mut sub = mailbox.subscribe("topic-a");
    let from = SandboxId::new();
    mailbox.publish("topic-b", from, json!("ignored")).unwrap();
    // try_recv should be Empty.
    assert!(sub.try_recv().unwrap().is_none());
}
