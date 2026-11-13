//! # Session Storage Stress Test
//!
//! End-to-end validation of JSONL transcript persistence, tombstones,
//! round-trip integrity, and write/load speed benchmarks.
//!
//! ```bash
//! cargo run -p bricks --example stress_memory --release
//! ```

use bricks::prelude::*;
use bricks_memory::session_storage::*;
use std::time::Instant;

fn main() {
    let mut passed = 0u32;
    let mut failed = 0u32;

    macro_rules! check {
        ($name:expr, $cond:expr) => {
            if $cond {
                passed += 1;
                println!("  \x1b[32m✓\x1b[0m {}", $name);
            } else {
                failed += 1;
                println!("  \x1b[31m✗\x1b[0m {}", $name);
            }
        };
    }

    println!("\n╔════════════════════════════════════════════════════╗");
    println!("║  Session Storage Stress Test                       ║");
    println!("╚════════════════════════════════════════════════════╝\n");

    // ── 1. Session Storage ───────────────────────────────────────────────
    println!("  1. Session Storage (JSONL)");
    println!("  ─────────────────────────");
    {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("session.jsonl");

        let u1 = write_user_entry(&path, "s1", Message::user("Hello"), "/tmp").unwrap();
        let _a1 = write_assistant_entry(
            &path,
            "s1",
            Message::assistant("Hi there!"),
            "/tmp",
            Some(&u1),
        )
        .unwrap();
        let u2 = write_user_entry(&path, "s1", Message::user("Fix the bug"), "/tmp").unwrap();
        let a2 =
            write_assistant_entry(&path, "s1", Message::assistant("Done!"), "/tmp", Some(&u2))
                .unwrap();
        let u3 = write_user_entry(&path, "s1", Message::user("Delete this"), "/tmp").unwrap();

        let entries = load_transcript(&path).unwrap();
        check!("5 entries written", entries.len() == 5);

        tombstone_entry(&path, &u3).unwrap();
        let entries = load_transcript(&path).unwrap();
        check!("Tombstone removes 1 entry", entries.len() == 4);

        let messages = messages_from_transcript(&entries);
        check!("4 messages extracted", messages.len() == 4);
        check!("First message correct", messages[0].get_text().unwrap() == "Hello");
        check!("Last message correct", messages[3].get_text().unwrap() == "Done!");
        check!(
            "Tombstoned message gone",
            messages
                .iter()
                .all(|m| m.get_text().unwrap() != "Delete this")
        );

        tombstone_entry(&path, &a2).unwrap();
        let entries = load_transcript(&path).unwrap();
        check!("Multiple tombstones work", entries.len() == 3);

        let messages = messages_from_transcript(&entries);
        for msg in &messages {
            check!(
                &format!("Message has text: '{}'", msg.get_text().unwrap()),
                msg.get_text().is_some()
            );
        }

        let summary = TranscriptEntry::Summary(SummaryEntry {
            uuid: "sum-1".into(),
            timestamp: chrono::Utc::now().to_rfc3339(),
            session_id: "s1".into(),
            summary: "User fixed a bug.".into(),
            messages_compacted: 3,
        });
        write_transcript_entry(&path, &summary).unwrap();
        let entries = load_transcript(&path).unwrap();
        check!("Summary entry persists", entries.len() == 4);
        println!();
    }

    // ── 2. Performance ───────────────────────────────────────────────────
    println!("  2. Performance Benchmarks");
    println!("  ────────────────────────");
    {
        let tmp = tempfile::tempdir().unwrap();
        let session_path = tmp.path().join("perf.jsonl");
        let start = Instant::now();
        for i in 0..100 {
            write_user_entry(
                &session_path,
                "perf",
                Message::user(format!("Message {}", i)),
                "/tmp",
            )
            .unwrap();
        }
        let write_us = start.elapsed().as_micros() / 100;
        check!(
            &format!("Session write: {}μs/entry (target < 500μs)", write_us),
            write_us < 500
        );

        let start = Instant::now();
        for _ in 0..10 {
            load_transcript(&session_path).unwrap();
        }
        let load_us = start.elapsed().as_micros() / 10;
        check!(
            &format!("Session load (100 entries): {}μs (target < 5000μs)", load_us),
            load_us < 5000
        );
        println!();
    }

    println!("╔════════════════════════════════════════════════════╗");
    if failed == 0 {
        println!(
            "║  \x1b[32mALL {} CHECKS PASSED\x1b[0m                                ║",
            passed
        );
    } else {
        println!(
            "║  \x1b[31m{} PASSED, {} FAILED\x1b[0m                                 ║",
            passed, failed
        );
    }
    println!("╚════════════════════════════════════════════════════╝\n");
    if failed > 0 {
        std::process::exit(1);
    }
}
