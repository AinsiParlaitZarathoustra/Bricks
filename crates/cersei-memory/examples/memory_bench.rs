//! Memory I/O: the operations of the README's "Memory I/O" table, on the
//! current API. Local files and an in-process Grafeo graph; no network.
//!
//! ```text
//! cargo run --release -p cersei-memory --features graph --example memory_bench
//! ```
//!
//! Each operation is timed over many iterations after a warm-up; the table
//! gives the mean, min and max per operation.

use cersei_memory::manager::MemoryManager;
use cersei_memory::memdir::{load_memory_index, scan_memory_dir, MemoryType};
use cersei_types::Message;
use std::time::{Duration, Instant};

fn measure(iters: usize, mut f: impl FnMut()) -> (Duration, Duration, Duration) {
    for _ in 0..3 {
        f();
    }
    let mut times = Vec::with_capacity(iters);
    for _ in 0..iters {
        let t = Instant::now();
        f();
        times.push(t.elapsed());
    }
    let total: Duration = times.iter().sum();
    (
        total / iters as u32,
        *times.iter().min().unwrap(),
        *times.iter().max().unwrap(),
    )
}

fn show(name: &str, (avg, min, max): (Duration, Duration, Duration), per: &str) {
    let f = |d: Duration| {
        let us = d.as_secs_f64() * 1e6;
        if us >= 1000.0 {
            format!("{:.2}ms", us / 1000.0)
        } else {
            format!("{us:.1}µs")
        }
    };
    println!("| {name} | {} | {} | {} | {per} |", f(avg), f(min), f(max));
}

fn main() {
    let dir = tempfile::tempdir().unwrap();
    let mem_dir = dir.path().join("memory");
    let sessions = dir.path().join("sessions");
    std::fs::create_dir_all(&mem_dir).unwrap();

    // 100 memory files with frontmatter, and a MEMORY.md index.
    let mut index = String::from("# Memory index\n\n");
    for i in 0..100 {
        let body = format!(
            "---\nname: note-{i}\ndescription: preference number {i}\ntype: user\n---\n\nThe user prefers option {i} for topic {}. Rust and tests.\n",
            i % 7
        );
        std::fs::write(mem_dir.join(format!("note-{i}.md")), body).unwrap();
        index.push_str(&format!("- [note {i}](note-{i}.md) — preference {i}\n"));
    }
    std::fs::write(mem_dir.join("MEMORY.md"), &index).unwrap();

    println!(
        "build: {}, {} {}",
        if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        },
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    println!("| operation | mean | min | max | unit |");
    println!("|---|---|---|---|---|");

    show(
        "Scan 100 files (frontmatter)",
        measure(200, || {
            assert_eq!(scan_memory_dir(&mem_dir).len(), 100);
        }),
        "per scan",
    );
    show(
        "Load MEMORY.md",
        measure(2000, || {
            assert!(load_memory_index(&mem_dir).is_some());
        }),
        "per load",
    );

    // Text recall: no graph, memdir scan + match (the fallback path).
    let text = MemoryManager::new(dir.path())
        .with_memory_dir(mem_dir.clone())
        .with_sessions_dir(sessions.clone());
    show(
        "Memory recall (text, 100 files)",
        measure(200, || {
            let _ = text.recall("option 42", 5);
        }),
        "per query",
    );

    // Graph: 1 000 memories stored, then recall and topic queries.
    let graph = MemoryManager::new(dir.path())
        .with_memory_dir(mem_dir.clone())
        .with_sessions_dir(sessions.clone())
        .with_graph_in_memory()
        .unwrap();
    let mut ids = Vec::new();
    let t = Instant::now();
    for i in 0..1000 {
        let id = graph
            .store_memory(
                &format!("User prefers option {i} for topic {}", i % 7),
                MemoryType::User,
                0.9,
            )
            .unwrap();
        if i % 10 == 0 {
            graph.tag_memory(&id, &format!("topic-{}", i % 7));
        }
        ids.push(id);
    }
    let store = t.elapsed() / 1000;
    show(
        "Graph store (1 000 nodes)",
        (store, store, store),
        "per node (mean of 1 000)",
    );
    show(
        "Memory recall (graph, 1 000 nodes)",
        measure(500, || {
            let _ = graph.recall("option 42", 5);
        }),
        "per query",
    );
    show(
        "Topic query (graph)",
        measure(500, || {
            let _ = graph.by_topic("topic-3");
        }),
        "per query",
    );

    // Sessions (JSONL, append-only).
    let mut n = 0;
    show(
        "Session write",
        measure(1000, || {
            n += 1;
            graph
                .write_user_message("bench-write", Message::user(format!("message {n}")))
                .unwrap();
        }),
        "per entry",
    );
    for i in 0..100 {
        graph
            .write_user_message("bench-load", Message::user(format!("message {i}")))
            .unwrap();
    }
    show(
        "Session load (100 entries)",
        measure(200, || {
            assert_eq!(
                graph.load_session_messages("bench-load").unwrap().len(),
                100
            );
        }),
        "per load",
    );
}
