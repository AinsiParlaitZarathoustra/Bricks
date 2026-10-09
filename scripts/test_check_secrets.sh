#!/usr/bin/env bash
# test_check_secrets.sh — check_secrets.py finds synthetic secrets, never
# prints them, honours its allowlist entry by entry, and passes the tree.
set -euo pipefail
cd "$(dirname "$0")/.."
TMP="$(mktemp -d "$PWD/.secret-check-test.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }

# Synthetic values, built at run time (none is written in the repository).
google="AIza$(printf 'Q%.0s' $(seq 1 35))"
openai="sk-$(printf 'x%.0s' $(seq 1 30))"
printf 'url=?key=%s\nconst K: &str = "%s";\n' "$google" "$openai" > "$TMP/leak.txt"

out="$(python3 scripts/check_secrets.py "$TMP/leak.txt" 2>&1)" && fail "not detected"
echo "$out" | grep -q 'google-api-key' || fail "google key missed: $out"
echo "$out" | grep -q 'openai-key' || fail "openai key missed: $out"
case "$out" in *"$google"*|*"$openai"*) fail "a value was printed" ;; esac

# Outside the repository: refused, not read.
python3 scripts/check_secrets.py /etc/hosts >/dev/null 2>&1 && fail "read a file outside the repository"

# The tracked tree passes with its allowlist.
python3 scripts/check_secrets.py --tracked >/dev/null || fail "the tracked tree has findings"
echo "check_secrets: all checks passed"
