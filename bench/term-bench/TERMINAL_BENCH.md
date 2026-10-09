# Running Abstract on Terminal-Bench 2.0

> Historical: this page describes the `abstract` harness of the Cersei era
> (`abstract_tbench.py`, `run_dry_tb.sh`, `run_tb_full.sh`). The current
> agent is `tbench-agent` (`tbench_agent.py`, `run_tb_cersei.sh`). Scripts
> live in `bench/term-bench/`.

## Prerequisites

1. **Docker** — required for sandboxed containers
2. **uv** — `curl -LsSf https://astral.sh/uv/install.sh | sh`
3. **API key** — set for the model's provider

## Setup

```bash
cd bench
uv sync
```

This installs `harbor` (the terminal-bench 2.0 harness) from `pyproject.toml`.

No harbor patching needed — the agent adapter lives in `bench/abstract_tbench.py`
and is loaded at runtime via `--agent-import-path`.

## Running the Benchmark

### Quick smoke test (single task)

```bash
OPENAI_API_KEY=<key> ./bench/run_dry_tb.sh

# With a different model
GOOGLE_API_KEY=<key> ./bench/run_dry_tb.sh --model google/gemini-3.1-pro-preview

# Specific task
./bench/run_dry_tb.sh --task terminal-bench/fibonacci-server
```

### Full run (terminal-bench@2.0, 89 tasks)

```bash
OPENAI_API_KEY=<key> ./bench/run_tb_full.sh

# With Claude, fewer concurrent tasks
ANTHROPIC_API_KEY=<key> ./bench/run_tb_full.sh \
    --model anthropic/claude-sonnet-4-6 \
    --concurrent 4
```

### Manual harbor command

```bash
cd bench
PYTHONPATH=. uv run harbor run \
    --agent-import-path "abstract_tbench:AbstractAgent" \
    --model openai/gpt-5.4-2026-03-05 \
    --dataset terminal-bench@2.0 \
    --n-concurrent 8 \
    --timeout-multiplier 2.5 \
    -y
```

### Key flags

| Flag | Description |
|------|-------------|
| `--model <provider/model>` | Model (e.g. `openai/gpt-5.4-2026-03-05`) |
| `--task <org/name>` | Run single task from registry |
| `--concurrent <N>` | Parallel tasks (default: 8) |
| `--timeout-mult <N>` | Timeout multiplier (default: 2.5) |
| `--include <pattern>` | Include task name pattern (glob) |
| `--exclude <pattern>` | Exclude task name pattern (glob) |
| `--attempts <N>` | Retries per task (for pass@k) |
| `--debug` | Enable debug logging |

## How it works

1. Harbor spins up a Docker container per task
2. `abstract_tbench.py` uploads the pre-built Linux binary via `upload_file`
3. Abstract runs with `--headless --no-permissions --output-format stream-json`
4. Harbor runs the task's verifier to check pass/fail

### What `--headless` does

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

For quick local testing:

```bash
OPENAI_API_KEY=<key> ./bench/run.sh --model openai/gpt-5.4-2026-03-05
./bench/report.sh
```
