#!/usr/bin/env bash
# Bricks — run Rust tests with nextest.
# Usage: scripts/run_tests.sh [--stress] [--graph] [--verbose]
set -euo pipefail
cd "$(dirname "$0")/.."

stress=false
features=()
output=()
for arg in "$@"; do
    case "$arg" in
        --stress) stress=true ;;
        --graph) features=(--features graph) ;;
        --verbose) output=(--success-output immediate) ;;
        *) echo "Unknown option: $arg" >&2; exit 2 ;;
    esac
done

cargo nextest run --workspace --locked "${features[@]}" "${output[@]}"
if "$stress"; then
    for example in stress_core_infrastructure stress_tools stress_skills stress_memory; do
        cargo run --locked --release -p bricks --example "$example" "${features[@]}"
    done
fi
cargo check --workspace --all-targets --locked "${features[@]}"
