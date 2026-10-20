# Bricks Benchmark Suite

Measures raw tool I/O performance and memory operations of the Bricks ADK. The
standalone suite is a separate package, so its build settings can differ from the
workspace's.

## Run

```bash
# Standalone suite (100 iterations per tool, Markdown output), from this folder
cd examples/benchmark
cargo run --release

# Quick version as an example of the bricks package (50 iterations), from the repository root
cargo run -p bricks --example benchmark_io --release

# Token and cost usage report, from the repository root
cargo run -p bricks --example usage_report --release
```

## What it measures

### Tool I/O (per-tool dispatch speed)

| Test | Description |
|------|-------------|
| **Read** | Read a 50 KB file with line-number formatting |
| **Write** | Write a 1 KB file to disk |
| **Edit** | Replace repeated text in a file (read, find, replace, write) |
| **Glob** | Find `**/*.rs` files across 20 files |
| **Grep** | Regex search across 20 files, through `rg` or `grep` |
| **Bash** | Run `sh -c "echo hello && ls -la"` and capture the output |

### Memory I/O

| Test | Description |
|------|-------------|
| **MEMORY.md load** | Load and parse the memory index file |
| **Scan 100 files** | Recursive scan of `.md` files with frontmatter parsing |
| **Session write** | Append one JSONL entry to a transcript |
| **Session load** | Two-pass load of 100 entries, with tombstone filtering |
| **Recall** | Text-match search across 100 memory files |

### Baselines

| Test | Description |
|------|-------------|
| **std::fs::read** | Raw standard-library file read |
| **std::fs::write** | Raw standard-library file write |

## Results

The figures are from the benchmark report of **8 October 2026**, which is kept in the repository as `docs/benchmarks-bricks-2026-10-08.md`. They were measured on Bricks 0.4.6 at commit `dee981e`, on Apple A18 Pro with macOS 27.2, in release mode.

| Tool | Mean | Min | Max |
|------|-----:|----:|----:|
| Read | 0.118 ms | 0.111 ms | 0.378 ms |
| Write | 0.050 ms | 0.037 ms | 0.290 ms |
| Edit | 0.759 ms | 0.700 ms | 1.830 ms |
| Glob | 0.066 ms | 0.046 ms | 0.230 ms |
| Grep | 2.017 ms | 1.365 ms | 4.765 ms |
| Bash | 39.695 ms | 35.799 ms | 84.883 ms |

The report predates the change that runs each `Bash` call in its own process (commit `4263cd9`), so the Bash figure does not describe the current code. For the memory operations, see the same report.

Results depend on the machine, the file system and the other programs running. Compare runs made the same way.
