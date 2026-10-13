#!/usr/bin/env bash
# ─── AgentRL run: Abstract (AgentRL mode) on Terminal-Bench 4.0 ───────────
#
# Thin wrapper over run_tb_full.sh that passes --agentrl (AgentRL self-evolving
# mode; the pipeline is implemented in crates/cersei-agentrl, whether the historical
# abstract binary runs it is not verifiable here) and pins Gemini 3.1 Pro.
# All run_tb_full.sh flags pass through.
#
# Usage:
#   ./run_tb_agentrl.sh                          # full dataset, Gemini 3.1 Pro
#   ./run_tb_agentrl.sh --task <name>            # single task
#   ./run_tb_agentrl.sh --local                  # local Docker instead of Daytona
#   ABSTRACT_AGENTRL_SAMPLES=3 ./run_tb_agentrl.sh   # best-of-3 general attempts
#
# Tuning env vars (forwarded into the container by abstract_tbench.py; the
# defaults below are not verifiable here, the binary being historical):
#   ABSTRACT_AGENTRL_SAMPLES    best-of-N general attempts (default 1)
#   ABSTRACT_AGENTRL_PROPOSALS  proposals per recovery round (default 2)
#   ABSTRACT_AGENTRL_ROUNDS     max recovery rounds (default 1)

set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

MODEL="${MODEL:-google/gemini-3.1-pro-preview}"
exec "$SCRIPT_DIR/run_tb_full.sh" --agentrl --model "$MODEL" "$@"
