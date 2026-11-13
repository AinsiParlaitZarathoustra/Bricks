# Bricks 0.4.6 Benchmarks (2026-10-08)

Measurements recorded on checkout `dee981e` with local benchmark harness fixes. The engine code itself was not modified for these runs.

- Machine: Apple A18 Pro, 6 cores, 8 GiB RAM, macOS 27.2, ARM64.
- Rust: 1.98.1; Python for frameworks: 3.12.13.
- Workspace build: release, `opt-level = "z"`, thin LTO, stripped symbols.
- Standalone build: release with `CARGO_PROFILE_RELEASE_OPT_LEVEL=z` and `CARGO_PROFILE_RELEASE_LTO=thin`.
- Sequential timed runs with no concurrent builds. The Mac stayed in normal use with background system processes including iCloud.
- LongMemEval, Terminal-Bench, and semantic search were excluded.
- No calls to a live model. Agents were built without inference; the token report uses a simulated provider.

## CLI Startup

50 runs of `--version` after 3 warmup runs. Binary size is the executable file size on disk, and memory is peak resident set size (RSS) for one run. Sizes and RSS use MiB (divided by 1024 squared).

| CLI | Version | Mean | Min - max | Binary | Peak RSS |
|---|---|---:|---:|---:|---:|
| Bricks | 0.4.6 | 4.8 ms | 4.3 - 6.1 ms | 29.4 MiB | 7.5 MiB |
| Claude Code | 2.1.289 | 7.3 ms | 6.4 - 11.2 ms | 219.0 MiB | 24.6 MiB |
| Codex CLI | 0.150.1 | 8.6 ms | 7.5 - 11.9 ms | 218.4 MiB | 16.9 MiB |

These measurements do not cover prompt response latency.

## Engine Tools

Standalone suite, 100 iterations per tool after warmup using temporary files. All tool runs were verified to succeed.

| Tool | Mean | Min | Max |
|---|---:|---:|---:|
| Read | 0.118 ms | 0.111 ms | 0.378 ms |
| Write | 0.050 ms | 0.037 ms | 0.290 ms |
| Edit | 0.759 ms | 0.700 ms | 1.830 ms |
| Glob | 0.066 ms | 0.046 ms | 0.230 ms |
| Grep | 2.017 ms | 1.365 ms | 4.765 ms |
| Bash | 39.695 ms | 35.799 ms | 84.883 ms |
| std::fs::read | 0.010 ms | 0.010 ms | 0.012 ms |
| std::fs::write | 0.045 ms | 0.028 ms | 0.165 ms |

The quick benchmark also exited with code 0. Across its 50 iterations, the mean times are:

| Read | Write | Edit | Glob | Grep | Bash |
|---:|---:|---:|---:|---:|---:|
| 0.11 ms | 0.05 ms | 0.81 ms | 0.05 ms | 1.86 ms | 39.82 ms |

`Edit` replaces all occurrences of the repeated text in the file. Older numbers timed an ambiguous call whose error was ignored, so they are not directly comparable to this fixed scenario.

Historical ratios between an internal read and `claude --help` are omitted because those two commands do not perform the same work.

## Local Memory

`memory_bench` suite with warmup and temporary files. These are local file and graph measurements, not LongMemEval.

| Operation | Mean | Min | Max |
|---|---:|---:|---:|
| Scan 100 files with frontmatter | 1.84 ms | 1.46 ms | 3.69 ms |
| Load MEMORY.md | 15.4 us | 12.9 us | 118.9 us |
| Text recall, 100 files | 2.03 ms | 1.58 ms | 4.20 ms |
| Graph write, 1,000 nodes | 76.2 us/node | - | - |
| Graph recall, 1,000 nodes | 1.36 ms | 1.18 ms | 4.99 ms |
| Topic query | 114.5 us | 104.6 us | 145.3 us |
| Session write | 35.9 us/entry | 25.5 us | 383.7 us |
| Session load, 100 entries | 279.3 us | 237.7 us | 381.5 us |

## Agent Frameworks

The `bricks-*` crates power the Bricks engine. The harness builds a minimal agent with one tool and runs no model turns. It measures 1,000 instantiations, then memory for 1,000 Rust agents or 500 Python agents. Python package versions come from `uv.lock` in the same environment.

| Framework | Version | Mean build | Measured allocation per agent |
|---|---|---:|---:|
| Bricks | 0.4.6 | 21.33 us | 71,703 bytes |
| Agno | 2.5.17 | 6.02 us | 5,394 bytes |
| LangGraph | 1.1.8 | 1,950.65 us | 30,344 bytes |
| PydanticAI | 1.22.0 | 228.62 us | 8,196 bytes |
| CrewAI | 1.14.2 | 7,476.15 us | 17,900 bytes |

Rust memory is measured with jemalloc; Python uses tracemalloc, which does not count all native allocations. The allocation columns are not an equivalent total-RAM comparison.

Concurrent build and live retention of 1,000 agents:

| Framework | Total time | Peak process RSS |
|---|---:|---:|
| Bricks | 20.43 ms | 98.5 MiB |
| Agno | 19.0 ms | 103.6 MiB |
| LangGraph | 2,101.1 ms | 168.7 MiB |
| PydanticAI | 262.1 ms | 107.2 MiB |
| CrewAI | 3,024.0 ms | 1,526.2 MiB |

RSS includes the runtime, imports, and peaks from earlier phases. Individual Rust latencies exclude wait time before task start; Python latencies include `asyncio.to_thread` wait time. Individual p50/p99 ratios are not an equivalent comparison.

Bricks also reached the **10,000 agents** tier: 197.37 ms total build time, 647.3 MiB peak RSS, 0.066 ms p50 individual build, and 1.048 ms p99. This is the largest tier tested, not a discovered upper limit.

Graph recall on 10,000 nodes: 10 workers x 100 queries (1,000 samples). Mean 87.99 ms, p50 83.03 ms, p95 130.01 ms, p99 226.49 ms. The binary banner still prints 100 agents, while the code runs 10 workers. The 100,000-node tier was not run; its zeroed JSON fields have `samples = 0` and are not measurements. Axis 5 (semantic search) is disabled.

## Stress Tests and Token Tracking

| Suite | Passed checks | Failures |
|---|---:|---:|
| Infrastructure | 46 | 0 |
| Tools | 47 | 0 |
| Orchestration | 33 | 0 |
| Skills | 47 | 0 |
| Memory | 83 | 0 |
| Total | 256 | 0 |

Simulated token report: 5,017 input tokens, 714 output tokens, two successful tool calls (`Glob`, `Read`). All three consistency checks pass: token counts, simulated cost tracking, and call count. These tokens and rates are defined by the simulator and do not measure real usage or model quality.

## Harness Fixes

- The top-level runner now starts from the repository root and targets Cargo packages explicitly. Pipeline failures propagate.
- Tool benchmarks check for errors and use `replace_all` in the repeated Edit scenario.
- The quick benchmark prints a signed duration difference without panicking when the raw measurement is slower.
- The tools suite expects the six current shell tools; its speed thresholds were not raised.
- The simulated provider assigns names and IDs to tool calls, uses a small temporary project, and verifies success.
- The Python benchmarks accept `CERSEI_BENCH_OUT_DIR` (the current environment variable in `bench/general-agents/_common.py`) to keep older results in `bench/general-agents/results/`.

## Reproduction and Outputs

Raw outputs from this run live in `target/benchmarks/2026-10-08/`, with comparison JSON files in `general-agents/`.

```sh
cargo run --release -p bricks --example benchmark_io
cargo run --release -p bricks-memory --features structured --example measure_memory
CERSEI_BENCH_AXES=1,2,3,4 cargo run --release -p bricks-agent \
  --example general_agent_bench --features bench-full
python3 scripts/bench_cli.py --bricks target/release/bricks --iterations 50
CARGO_PROFILE_RELEASE_OPT_LEVEL=z CARGO_PROFILE_RELEASE_LTO=thin \
  cargo run --release --manifest-path examples/benchmark/Cargo.toml
```

The five `stress_*` examples and `usage_report` run with `cargo run --release -p bricks --example NAME`.
For Python, from `bench/general-agents`, install with `uv sync --locked --extra all --python 3.12`, then run `.venv/bin/python bench_agno.py`, `bench_langgraph.py`, `bench_pydantic_ai.py`, and `bench_crewai.py`. Model calls are not part of these suites.

The older `abstract` comparison scripts were removed in Sprint 15. Their startup, binary size, and RSS measurements are covered here by `bench_cli.py` with Bricks. Their historical subcommands and agentic scenarios with live models were not ported or run.
