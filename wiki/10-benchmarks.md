# Benchmarks

The figures on this page come from the benchmark report of **2026-10-08** (`docs/benchmarks-bricks-2026-10-08.md`), measured on Bricks 0.4.6 (commit `dee981e`) on an Apple A18 Pro (6 cores, 8 GiB RAM, macOS 27.2, Rust 1.98.1, release build with `opt-level = "z"`, thin LTO, stripped symbols). No live LLM calls were made during these runs.

Note that some subsystems changed after 2026-10-08 (for example, `Grep` now runs in-process, `Bash` runs each call in its own process, and `bricks-memory` removed legacy file/graph modules in favor of structured memory). Re-run the commands below to measure your own hardware and build.

## How to Run

From the repository root:

```bash
# Quick tool I/O benchmark (50 iterations)
cargo run --release -p bricks --example benchmark_io

# Standalone tool I/O suite (100 iterations)
cargo run --release --manifest-path examples/benchmark/Cargo.toml

# Stress tests
cargo run --release -p bricks --example stress_core_infrastructure
cargo run --release -p bricks --example stress_tools
cargo run --release -p bricks --example stress_skills
cargo run --release -p bricks --example stress_memory

# Simulated token and cost tracking check
cargo run --release -p bricks --example usage_report
```

## CLI Startup

50 runs of `--version` after 3 warm-up runs (`docs/benchmarks-bricks-2026-10-08.md`). Binary size and max RSS are in MiB.

| CLI | Version | Mean | Min - Max | Binary | Max RSS |
|-----|---------|-----:|----------:|-------:|--------:|
| Bricks | 0.4.6 | 4.8 ms | 4.3 - 6.1 ms | 29.4 MiB | 7.5 MiB |
| Claude Code | 2.1.289 | 7.3 ms | 6.4 - 11.2 ms | 219.0 MiB | 24.6 MiB |
| Codex CLI | 0.150.1 | 8.6 ms | 7.5 - 11.9 ms | 218.4 MiB | 16.9 MiB |

These numbers measure binary startup only, not prompt response time.

## Tool I/O Performance

Standalone suite, 100 iterations per tool in release mode (`docs/benchmarks-bricks-2026-10-08.md`):

| Tool | Mean | Min | Max |
|------|-----:|----:|----:|
| Read | 0.118 ms | 0.111 ms | 0.378 ms |
| Write | 0.050 ms | 0.037 ms | 0.290 ms |
| Edit | 0.759 ms | 0.700 ms | 1.830 ms |
| Glob | 0.066 ms | 0.046 ms | 0.230 ms |
| Grep | 2.017 ms | 1.365 ms | 4.765 ms |
| Bash | 39.695 ms | 35.799 ms | 84.883 ms |
| `std::fs::read` (baseline) | 0.010 ms | 0.010 ms | 0.012 ms |
| `std::fs::write` (baseline) | 0.045 ms | 0.028 ms | 0.165 ms |

## Agent Instantiation

Building a minimal agent with one tool and no model inference (`docs/benchmarks-bricks-2026-10-08.md`):

| Framework | Version | Mean build time | Concurrent 1,000 agents | Max RSS (1,000 agents) |
|-----------|---------|----------------:|------------------------:|-----------------------:|
| Bricks | 0.4.6 | 21.33 us | 20.43 ms | 98.5 MiB |
| Agno | 2.5.17 | 6.02 us | 19.0 ms | 103.6 MiB |
| LangGraph | 1.1.8 | 1,950.65 us | 2,101.1 ms | 168.7 MiB |
| PydanticAI | 1.22.0 | 228.62 us | 262.1 ms | 107.2 MiB |
| CrewAI | 1.14.2 | 7,476.15 us | 3,024.0 ms | 1,526.2 MiB |

Bricks also completed 10,000 concurrent agent builds in 197.37 ms (647.3 MiB max RSS, p50 0.066 ms, p99 1.048 ms).

## Stress Checks

Results from the 2026-10-08 report (`docs/benchmarks-bricks-2026-10-08.md`):

| Suite | Passed | Failed |
|-------|-------:|-------:|
| Infrastructure | 46 | 0 |
| Tools | 47 | 0 |
| Orchestration | 33 | 0 |
| Skills | 47 | 0 |
| Memory | 83 | 0 |
| **Total** | **256** | **0** |

## Context Compaction Thresholds

Verified in `crates/bricks-agent/src/compact.rs`:

| Context used | Action | Constant |
|---|---|---|
| < 80% | Normal operation | `WARNING_PCT = 0.80` |
| 80% to 90% | Warning emitted | `WARNING_PCT` |
| 90%+ | Auto-compact triggers | `AUTOCOMPACT_TRIGGER_FRACTION = 0.90` |
| 95%+ | Critical warning emitted | `CRITICAL_PCT = 0.95` |
| 98%+ | Emergency context collapse | `should_context_collapse` (`>= 0.98`) |

After `MAX_CONSECUTIVE_FAILURES = 3` failed compactions, `AutoCompactState::on_failure` disables auto-compact until `AutoCompactState::on_success` resets the counter.
