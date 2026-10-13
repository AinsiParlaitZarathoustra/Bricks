# Running Abstract on Terminal-Bench 4.0

> Historical: this page describes the `abstract` harness of the Cersei era
> (`abstract_tbench.py`, `run_dry_tb.sh`, `run_tb_full.sh`). The current
> agent is `tbench-agent` (`tbench_agent.py`, `run_tb_cersei.sh`). Scripts
> live in `bench/term-bench/`.

## Prerequisites

1. **Docker** — for local runs (`--local`). The `run_*.sh` scripts run on
   Daytona by default and need `DAYTONA_API_KEY` in `bench/term-bench/.env`
2. **uv** — `curl -LsSf https://astral.sh/uv/install.sh | sh`
3. **API key** — set for the model's provider

## Setup

```bash
cd bench/term-bench
uv sync
```

This installs `harbor` (locked at 0.3.0 in `uv.lock`) from
`bench/term-bench/pyproject.toml`.

No harbor patching needed — the agent adapter lives in
`bench/term-bench/abstract_tbench.py` and is loaded at runtime via
`--agent-import-path`.

## Running the Benchmark

### Quick smoke test (single task)

```bash
# Default model is google/gemini-3.1-pro-preview
GOOGLE_API_KEY=<key> ./bench/term-bench/run_dry_tb.sh

# With a different model
OPENAI_API_KEY=<key> ./bench/term-bench/run_dry_tb.sh --model openai/gpt-5.4-2026-03-05

# Specific task
./bench/term-bench/run_dry_tb.sh --task terminal-bench/fibonacci-server
```

### Full run (terminal-bench@4.0.0)

```bash
OPENAI_API_KEY=<key> ./bench/term-bench/run_tb_full.sh --model openai/gpt-5.4-2026-03-05

# With Claude, fewer concurrent tasks
ANTHROPIC_API_KEY=<key> ./bench/term-bench/run_tb_full.sh \
    --model anthropic/claude-sonnet-4-6 \
    --concurrent 4
```

### Manual harbor command

```bash
cd bench/term-bench
PYTHONPATH=. uv run harbor run \
    --agent-import-path "abstract_tbench:AbstractAgent" \
    --model openai/gpt-5.4-2026-03-05 \
    --dataset terminal-bench@4.0.0 \
    --n-concurrent 8 \
    --timeout-multiplier 2.5 \
    -y
```

### Key flags

Flags of `run_tb_full.sh`; defaults are the script's own (`run_tb_cersei.sh`
uses 12 concurrent tasks and a 2.0 timeout multiplier).

| Flag | Description |
|------|-------------|
| `--model <provider/model>` | Model (default: `google/gemini-3.1-pro-preview`) |
| `--task <name>` | Run single task (passed as `--include-task-name`) |
| `--concurrent <N>` | Parallel tasks (default: 20) |
| `--timeout-mult <N>` | Timeout multiplier (default: 1.0) |
| `--include <pattern>` | Include task name pattern (glob) |
| `--exclude <pattern>` | Exclude task name pattern (glob) |
| `--attempts <N>` | Attempts per task, for pass@k (default: 1) |
| `--debug` | Enable debug logging |

## How it works

1. Harbor starts one sandbox per task (Daytona by default, Docker with `--local`)
2. `abstract_tbench.py` uploads the pre-built Linux binary via `upload_file`
3. Abstract runs with `--headless --no-permissions --output-format stream-json`
4. Harbor runs the task's verifier to check pass/fail

### What `--headless` does

Not verifiable in this repository: `--headless` belongs to the historical
`abstract` binary, whose source is not here. The list below is unchecked.

- Switches to a test-driven system prompt (reads /tests/run-tests.sh first)
- Enables self-verification loop (always runs tests, retries up to 4x on failure)
- Includes test error output in retry messages so the model learns from failures
- Auto-approves all tool permissions
- Increases max turns to 80

## Linux binaries

The binaries are not tracked by Git. How to build `tbench-agent`, the
checksums of the historical `abstract-*` binaries, how to recover them from
the history, and the environment variables that point the harnesses at
another path: see [BINARIES.md](BINARIES.md).

## Local benchmark (without Docker)

For quick local testing. Historical: `run.sh` runs the former `abstract`
binary (`target/release/abstract` by default, or `--bin <path>`), which is no
longer built by this workspace; see [BINARIES.md](BINARIES.md).

```bash
OPENAI_API_KEY=<key> ./bench/term-bench/run.sh --model openai/gpt-5.4-2026-03-05
./bench/term-bench/report.sh
```
