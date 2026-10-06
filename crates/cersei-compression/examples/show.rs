//! Print how one fixture or file is processed:
//! `cargo run -p cersei-compression --example show -- <file> <tool> <command-or-path> [level]`
use cersei_compression::{
    CompressionConfig, CompressionLevel, Compressor, RawStore, RuleSet, ToolOutput,
};
use std::sync::Arc;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let content = std::fs::read_to_string(&args[1]).expect("readable file");
    let tool = args.get(2).map(String::as_str).unwrap_or("Bash");
    let arg = args.get(3).map(String::as_str).unwrap_or("");
    let level: CompressionLevel = args
        .get(4)
        .map(|l| l.parse().unwrap())
        .unwrap_or(CompressionLevel::Minimal);
    let input = if tool == "Read" {
        serde_json::json!({ "file_path": arg })
    } else {
        serde_json::json!({ "command": arg })
    };
    let store = RawStore::new(std::env::temp_dir().join("bricks-show"));
    let c = Compressor::new(
        CompressionConfig::default(),
        Arc::new(RuleSet::builtin()),
        Some(store),
    );
    let p = c.process(
        &ToolOutput {
            tool,
            input: &input,
            content: &content,
            is_error: false,
            call_id: "show",
            exit_code: None,
        },
        level,
    );
    println!("{}", p.text);
    eprintln!("{:?}", p.stats);
}
