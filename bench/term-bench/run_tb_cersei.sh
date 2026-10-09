#!/usr/bin/env bash
# ─── cersei-tbench (purpose-built AgentRL agent) on Terminal-Bench 2.0 ─────
#
# Usage:
#   ./run_tb_cersei.sh                       # full 89 tasks, Gemini 3.1 Pro, Daytona
#   ./run_tb_cersei.sh --task <name>         # single task
#   ./run_tb_cersei.sh --local               # local Docker instead of Daytona
#   ./run_tb_cersei.sh --concurrent 10
#   TBENCH_SAMPLES=3 TBENCH_ROUNDS=1 ./run_tb_cersei.sh   # best-of-N + recovery
#
# Prereqs: uv; bench/.env with DAYTONA_API_KEY + model key; built tbench-agent binaries.

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ENV_FILE="$SCRIPT_DIR/.env"
[[ -f "$ENV_FILE" ]] && { set -a; source "$ENV_FILE"; set +a; }

MODEL="${MODEL:-vertex/claude-opus-4-8}"
CONCURRENT="${CONCURRENT:-12}"
DATASET="terminal-bench@2.0"
OUTPUT_DIR="$SCRIPT_DIR/tb-results"
JOB_NAME="cersei-tbench-$(date +%Y%m%d-%H%M%S)"
TIMEOUT_MULT="${TIMEOUT_MULT:-2.0}"
ATTEMPTS="${ATTEMPTS:-1}"
USE_DAYTONA=true
EXTRA_ARGS=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --model)        MODEL="$2"; shift 2 ;;
    --concurrent)   CONCURRENT="$2"; shift 2 ;;
    --timeout-mult) TIMEOUT_MULT="$2"; shift 2 ;;
    --local)        USE_DAYTONA=false; shift ;;
    --task)         EXTRA_ARGS+=("--include-task-name" "$2"); shift 2 ;;
    --include)      EXTRA_ARGS+=("--include-task-name" "$2"); shift 2 ;;
    --exclude)      EXTRA_ARGS+=("--exclude-task-name" "$2"); shift 2 ;;
    --attempts)     ATTEMPTS="$2"; shift 2 ;;
    --output)       OUTPUT_DIR="$2"; shift 2 ;;
    --help|-h)
      echo "Usage: $0 [--task name] [--concurrent N] [--local] [--attempts N]"
      echo "Env: TBENCH_SAMPLES (best-of-N), TBENCH_ROUNDS (recovery rounds)"
      exit 0 ;;
    *) echo "Unknown arg: $1"; exit 1 ;;
  esac
done

CYAN='\033[0;36m'; GREEN='\033[0;32m'; RED='\033[0;31m'; DIM='\033[0;90m'; BOLD='\033[1m'; RESET='\033[0m'
info(){ echo -e "${CYAN}▸${RESET} $*"; }; pass(){ echo -e "${GREEN}✓${RESET} $*"; }; fail(){ echo -e "${RED}✗${RESET} $*"; }

echo ""; echo -e "${BOLD}cersei-tbench × Terminal-Bench 2.0${RESET}"
command -v uv &>/dev/null && pass "uv" || { fail "'uv' not found"; exit 1; }
[[ -f "$SCRIPT_DIR/tbench_agent.py" ]] && pass "Adapter" || { fail "tbench_agent.py missing"; exit 1; }
[[ -f "${TBENCH_AGENT_BINARY_AMD64:-$SCRIPT_DIR/tbench-agent-linux-amd64}" || -f "${TBENCH_AGENT_BINARY_ARM64:-$SCRIPT_DIR/tbench-agent-linux-arm64}" ]] \
  && pass "Binary" || { fail "tbench-agent binary missing — build it as in bench/term-bench/BINARIES.md"; exit 1; }

ENV_FLAG=""
if $USE_DAYTONA; then
  [[ -n "${DAYTONA_API_KEY:-}" ]] && { ENV_FLAG="--env daytona"; pass "Daytona"; } || { fail "DAYTONA_API_KEY not set"; exit 1; }
else
  docker info &>/dev/null && { ENV_FLAG="--env docker"; pass "Docker"; } || { fail "Docker not running"; exit 1; }
fi

PROVIDER="${MODEL%%/*}"
if [[ "$PROVIDER" == "vertex" ]]; then
  # Vertex auth: project id + a service-account key (forwarded into containers).
  [[ -n "${VERTEX_PROJECT_ID:-}" ]] && pass "VERTEX_PROJECT_ID" || { fail "VERTEX_PROJECT_ID not set"; exit 1; }
  SA="${VERTEX_SA_FILE:-$SCRIPT_DIR/../../service-account.json}"
  [[ -f "$SA" ]] && pass "service-account.json" || { fail "service-account.json not found ($SA); set VERTEX_SA_FILE"; exit 1; }
  export VERTEX_PROJECT_ID VERTEX_LOCATION="${VERTEX_LOCATION:-global}" VERTEX_SA_FILE="$SA"
else
  case "$PROVIDER" in
    openai) KEY_ORDER=(OPENAI_API_KEY) ;;
    google) KEY_ORDER=(GOOGLE_API_KEY GEMINI_API_KEY) ;;
    anthropic) KEY_ORDER=(ANTHROPIC_API_KEY) ;;
    *) KEY_ORDER=(GOOGLE_API_KEY OPENAI_API_KEY ANTHROPIC_API_KEY) ;;
  esac
  HASK=false; for k in "${KEY_ORDER[@]}"; do [[ -n "${!k:-}" ]] && { HASK=true; pass "API key ($k)"; break; }; done
  $HASK || { fail "No API key for '$PROVIDER'"; exit 1; }
fi

echo -e "${DIM}  Model: $MODEL | Concurrent: $CONCURRENT | samples=${TBENCH_SAMPLES:-1} rounds=${TBENCH_ROUNDS:-0} | Job: $JOB_NAME${RESET}"
echo ""

mkdir -p "$OUTPUT_DIR"; cd "$SCRIPT_DIR"
info "Starting benchmark..."

PYTHONPATH="$SCRIPT_DIR${PYTHONPATH:+:$PYTHONPATH}" uv run harbor run \
    --agent-import-path "tbench_agent:CerseiTBenchAgent" \
    --model "$MODEL" \
    --dataset "$DATASET" \
    --n-concurrent "$CONCURRENT" \
    --jobs-dir "$OUTPUT_DIR" \
    --job-name "$JOB_NAME" \
    --timeout-multiplier "$TIMEOUT_MULT" \
    -k "$ATTEMPTS" \
    --max-retries 2 \
    --retry-include "NonZeroAgentExitCodeError" \
    --env-file "$ENV_FILE" \
    $ENV_FLAG \
    -y \
    ${EXTRA_ARGS[@]+"${EXTRA_ARGS[@]}"}

echo ""; pass "Done → $OUTPUT_DIR/$JOB_NAME"
