# Benchmarks

> **Two sources, read them separately.**
>
> - **Current:** Bricks 0.4.6, measured on 2026-10-08 (commit `dee981e`; Apple A18 Pro, 6 cores, 8 GiB, macOS 27.2, Rust 1.98.1). The full report is [`docs/benchmarks-bricks-2026-10-08.md`](../docs/benchmarks-bricks-2026-10-08.md), and the summary is in the [Benchmarks](../README.md#benchmarks) section of the README. There is no benchmark report for 0.6.0 in the repository.
> - **Historical:** the tables below are the "Latest Results" of [`examples/benchmark/README.md`](../examples/benchmark/README.md), a file whose only commit is the first one (`53b7aee`, 2026-04-02). They describe an earlier Cersei build. Do not compare them with the current numbers: Bash averages 15.64 ms there, and 39.695 ms in the 2026-10-08 report.
>
> Anything marked **[unverified]** does not appear in any file of the repository.

## How to Run

From the repository root:

```bash
# Tool I/O benchmark (quick)
cargo run --release -p cersei --example benchmark_io

# Standalone suite, with its own manifest
cargo run --release --manifest-path examples/benchmark/Cargo.toml

# Stress tests (correctness + performance)
cargo run --release -p cersei --example stress_core_infrastructure
cargo run --release -p cersei --example stress_tools
cargo run --release -p cersei --example stress_orchestration
cargo run --release -p cersei --example stress_skills
cargo run --release -p cersei --example stress_memory

# Usage and cost tracking
cargo run --release -p cersei --example usage_report
```

The Python agent-framework comparisons are in `bench/general-agents`. LongMemEval (`bench/long-mem`) and Terminal-Bench (`bench/term-bench`) need a dataset or a live model; the README lists them under "Not re-run yet".

---

## Methodology

Historical (`examples/benchmark/README.md`):

- **Machine**: Apple Silicon (macOS), release build
- **Iterations**: 100 per tool in the standalone suite, 50 for the quick benchmark
- **Scope**: raw tool I/O (per-tool dispatch speed)

Not stated in the repository **[unverified]**: 3 warm-up runs, the `std::time::Instant` clock, and a fresh temp directory per run.

---

## Tool I/O Performance

### Results (historical, 100 iterations, release build)

| Tool | Avg | Min | Max | What It Measures |
|------|-----|-----|-----|------------------|
| **Edit** | **0.04ms** | 0.02ms | 0.05ms | Read file + find string + replace + write back |
| **Glob** | **0.05ms** | 0.05ms | 0.07ms | Pattern match across 20 files |
| **Write** | **0.09ms** | 0.07ms | 0.11ms | Write 1KB to disk (creates parent dirs) |
| **Read** | **0.09ms** | 0.08ms | 0.11ms | Read 50KB file + format with line numbers |
| **Grep** | **5.85ms** | 5.34ms | 8.51ms | `rg` subprocess across 20 files |
| **Bash** | **15.64ms** | 14.50ms | 16.19ms | `sh -c "echo hello && ls -la"` |

### vs Raw `std::fs`

The README defines `std::fs::read` and `std::fs::write` as baselines but gives no numbers for them. The figures below are **[unverified]**. The 2026-10-08 report measures `std::fs::read` at 0.010 ms and `std::fs::write` at 0.045 ms, on another build.

| Operation | `std::fs` | Cersei | Overhead |
|-----------|-----------|--------|----------|
| Read | 0.012ms **[unverified]** | 0.09ms | +0.078ms (JSON parsing + line numbering) **[unverified]** |
| Write | 0.071ms **[unverified]** | 0.09ms | +0.019ms (JSON parsing + dir check) **[unverified]** |

### CLI startup: this ratio is not carried forward

The historical README compares a Read dispatch (0.09 ms) with `claude --help` (323 ms) and gives a ratio of 3,589x. The two operations are not the same, and the 2026-10-08 report does not reuse this comparison. Its current startup figures, for `--version` only, are:

| CLI | Version | Mean | Binary | Max RSS |
|-----|---------|------|--------|---------|
| Bricks | 0.4.6 | 4.8 ms | 29.4 MiB | 7.5 MiB |
| Claude Code | 2.1.289 | 7.3 ms | 219.0 MiB | 24.6 MiB |
| Codex CLI | 0.150.1 | 8.6 ms | 218.4 MiB | 16.9 MiB |

Those figures do not measure the response time to a prompt. The reasons usually given for the gap (Node.js start-up and similar) are **[unverified]**.

### What This Means for Agents

Computed from the 0.09 ms Read figure above; not measured as a whole:

| Scenario | Tool Calls | Cersei Overhead | Note |
|----------|-----------|----------------|------|
| Quick fix | 5 | 0.5ms | Invisible |
| Feature build | 50 | 4.5ms | Invisible |
| Large refactor | 200 | 18ms | Invisible |
| CI batch (1000 files) | 1000 | 90ms | Noticeable only in batch |

The LLM API latency per turn (500 ms to 5 s) is **[unverified]** here. Tool dispatch overhead matters mainly for high-frequency automation.

### Why Grep and Bash Are Slower

| Tool | Bottleneck | Why |
|------|-----------|-----|
| Grep (~6ms) | `rg` subprocess | The cost of process creation on macOS (~5 ms) and the in-process regex figure (~0.1 ms) are **[unverified]**. |
| Bash (~16ms) | `sh -c` subprocess | The 15.64 ms in the table above. The cost is a process spawn; it is not broken down here. |

---

## Memory I/O Performance

Historical, measured with 100 memory files (`examples/benchmark/README.md`):

| Operation | Speed | Target **[unverified]** | Headroom (computed) |
|-----------|-------|--------|----------|
| **MEMORY.md load** | **12μs** | <1000μs | 83x |
| **Session write** (per entry) | **51μs** | <500μs | 10x |
| **Session load** (100 entries) | **155μs** | <5000μs | 32x |
| **Scan 100 memory files** | **1,378μs** | <5000μs | 3.6x |
| **Recall from 100 files** | **1,482μs** | <10000μs | 6.7x |

### What This Means

- **Session writes at 51μs/entry** = about 20,000 writes per second (1 / 51 μs, computed).
- **Scanning 100 memory files takes 1.4 ms.** The claim that most projects have fewer than 20 memory files is **[unverified]**.
- **Loading MEMORY.md takes 12μs.**
- **Compatibility with the `~/.claude/projects/` layout** (tested against 42 project directories and 171 transcripts) is **[unverified]**: no file in the repository records this test.

---

## Token Consumption Estimates

Historical, simulated 4-turn session with 11,901 input and 2,110 output tokens (`examples/benchmark/README.md`). The README gives only session totals, so the per-turn breakdown below is **[unverified]**.

### Per-Session Breakdown **[unverified]**

| Turn | Input Tokens | Output Tokens | Cost (Sonnet) |
|------|-------------|---------------|---------------|
| 1 (file read) | 2,900 | 1,250 | $0.027 |
| 2 (file write) | 2,467 | 380 | $0.017 |
| 3 (verify) | 3,000 | 195 | $0.016 |
| 4 (summary) | 3,534 | 285 | $0.020 |
| **Total** | **11,901** | **2,110** | **$0.081** |

### Model Cost Comparison

Same session (11,901 in / 2,110 out). The totals are in `examples/benchmark/README.md`. The per-model input and output splits are **[unverified]**, and the per-model prices are not in the repository: `docs/providers.example.toml` says its prices are illustrative.

| Model | Total |
|-------|-------|
| Claude Sonnet 4.6 | **$0.067** |
| Claude Opus 4.6 | **$0.337** |
| Claude Haiku 4.5 | **$0.018** |

### Projected Monthly Costs

Computed from the Sonnet 4.6 total of $0.067 per session, 30 days a month:

| Usage | Daily Cost | Monthly Cost |
|-------|-----------|-------------|
| 10 sessions/day | $0.67 | $20 |
| 50 sessions/day | $3.35 | $101 |
| 100 sessions/day | $6.70 | $201 |

### Efficiency Metrics

| Metric | Value |
|--------|-------|
| Input/output ratio | 5.6:1 (computed: 11,901 / 2,110) |
| Cost per turn | about $0.017 (computed: $0.067 / 4 turns) |
| Cost per tool call | $0.027 **[unverified]** |
| Tokens/second (throughput) | ~25,000 **[unverified]** |

---

## Context Management Performance

### Auto-Compact Thresholds

The thresholds are constants in `crates/cersei-agent/src/compact.rs`. Their names are shown below. The "Validated" column of the historical page is **[unverified]**: no file records how each threshold was tested.

| Context used | Action | Constant |
|-----------|--------|-----------|
| <80% | No action | `WARNING_PCT = 0.80` (warning threshold) |
| 80-90% | Warning | `WARNING_PCT` |
| 90%+ | Auto-compact triggers | `AUTOCOMPACT_TRIGGER_FRACTION = 0.90` |
| 95%+ | Critical warning | `CRITICAL_PCT = 0.95` |
| 98%+ | Context collapse (emergency) | `should_context_collapse` (`>= 0.98`) |

### Tool Result Budget **[unverified]**

| Scenario | Before | After | Reduction |
|----------|--------|-------|-----------|
| 20 tool results (185KB) | 185,000 chars | 48,070 chars | 74% |
| Recent results | Preserved | Preserved | — |
| Truncated entries | — | 15 entries | "[truncated]" notice |

No file in the repository holds these figures.

### Compact Circuit Breaker

Verified in `crates/cersei-agent/src/compact.rs`:

- 3 consecutive failures (`MAX_CONSECUTIVE_FAILURES = 3`) disable auto-compact (`AutoCompactState::on_failure`).
- A successful compaction resets the counter (`AutoCompactState::on_success`).

---

## Stress Test Results

The check counts in the 2026-10-08 report are the current ones: Infrastructure 46, Tools 47, Orchestration 33, Skills 47, Memory 83, total 256, with no failure. The Skills (51) and Memory (85) figures below predate that report and do not match it. The per-subsystem rows in this section are **[unverified]**: no file in the repository records them.

### Core Infrastructure (46 checks)

| Subsystem | Checks | Status |
|-----------|--------|--------|
| System prompt builder | 19 | All pass |
| Bash classifier (23 commands) | 2 | All pass |
| Context analyzer | 8 | All pass |
| Auto-compact | 14 | All pass |
| Tool result budget | 3 | All pass |

### Tools (47 checks)

| Category | Tools | Checks | Status |
|----------|-------|--------|--------|
| Filesystem | Read, Write, Edit, Glob, Grep, NotebookEdit | 7 | All pass |
| Shell | Bash, PowerShell | 2 | All pass |
| Planning | PlanMode, TodoWrite | 4 | All pass |
| Scheduling | Cron (3), Sleep, RemoteTrigger | 6 | All pass |
| Orchestration | SendMessage | 3 | All pass |
| Config/Utility | Config, SyntheticOutput, AskUser | 3 | All pass |
| Web | WebFetch, WebSearch | 4 | Schema only |
| ToolSearch | 4 queries | 4 | All pass |
| Performance | Read, Edit, Glob (<500μs) | 3 | All pass |
| Registry | 24 tools, unique names, valid schemas | 10 | All pass |

### Orchestration (33 checks)

| Subsystem | Checks | Status |
|-----------|--------|--------|
| AgentTool (sub-agents) | 10 | All pass |
| Coordinator mode | 8 | All pass |
| Task system (6 tools) | 8 | All pass |
| Message passing | 5 | All pass |
| Full orchestration (3 workers) | 2 | All pass |

### Skills (51 checks, superseded: 47 in the 2026-10-08 report)

| Subsystem | Checks | Status |
|-----------|--------|--------|
| Bundled skills (7 skills) | 19 | All pass |
| Claude Code format | 7 | All pass |
| OpenCode format | 3 | All pass |
| Precedence & dedup | 2 | All pass |
| SkillTool integration | 6 | All pass |
| Real user skills | 3 | All pass |
| Formatting | 3 | All pass |

### Memory (85 checks, superseded: 83 in the 2026-10-08 report)

| Subsystem | Checks | Status |
|-----------|--------|--------|
| Memdir (scanning, FM, staleness) | 21 | All pass |
| CLAUDE.md (4 scopes, @include) | 10 | All pass |
| Session storage (JSONL, tombstones) | 10 | All pass |
| Session memory extraction | 8 | All pass |
| Auto-dream (3-gate system) | 9 | All pass |
| Graph memory (feature gate) | 2 | All pass |
| Unified manager | 13 | All pass |
| Performance (scan, recall, write, load) | 5 | All pass |
| Real user data compatibility | 1+ | All pass |

---

## Cumulative Totals

| Metric | Value | Status |
|--------|-------|--------|
| Unit tests | **160** | `CHANGELOG.md`, release 0.1.0 (2026-04-02) |
| Stress checks | **262** | `CHANGELOG.md`, release 0.1.0; the 2026-10-08 report has 256 |
| Tools | **30+** | **[unverified]** |
| Source files | **35+** | **[unverified]** |
| Crates | **9** | Historical: the workspace has 24 members (`Cargo.toml`) |
| I/O regressions | **0** | **[unverified]** |
| Test failures | **0** | **[unverified]**; the 2026-10-08 report shows 0 failures among its stress checks |
