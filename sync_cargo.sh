#!/usr/bin/env bash
#
# sync_cargo.sh — publish the library crates of the workspace to crates.io.
#
# It publishes the crates listed in CRATES below — an explicit selection, not
# every workspace member (`cersei-testkit` and `longmem-bench` are private;
# `bricks-cli` and `bricks-tui` are not published by this script) — at the
# version declared in the workspace `Cargo.toml`, in dependency order (leaves
# first), so crates.io always has a crate's dependencies before its dependents.
# Before anything is done, the list is checked against `cargo metadata`
# (scripts/check_publish_order.py): an unknown or private crate, a missing
# internal dependency or a wrong order stops the script. Crates whose current
# version is already on crates.io are skipped, so the script is safe to re-run
# after a partial/failed run.
#
# Usage:
#   ./sync_cargo.sh                 # publish everything that isn't already up
#   ./sync_cargo.sh --dry-run       # run `cargo publish --dry-run` for each
#   ./sync_cargo.sh --allow-dirty    # pass --allow-dirty to cargo publish
#
# Auth: set CARGO_REGISTRY_TOKEN, or run `cargo login` beforehand.

set -euo pipefail

cd "$(dirname "$0")"

DRY_RUN=0
ALLOW_DIRTY=0
for arg in "$@"; do
  case "$arg" in
    --dry-run)     DRY_RUN=1 ;;
    --allow-dirty) ALLOW_DIRTY=1 ;;
    *) echo "unknown flag: $arg" >&2; exit 2 ;;
  esac
done

# Seconds to wait after a successful publish, giving the crates.io index time to
# propagate before a dependent crate is published. Modern cargo also waits for
# the index itself, but this adds a safety margin.
PROPAGATION_SLEEP="${PROPAGATION_SLEEP:-15}"

# Version published for every crate — the single workspace version.
VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"([^"]+)".*/\1/')"
if [[ -z "$VERSION" ]]; then
  echo "could not read workspace version from Cargo.toml" >&2
  exit 1
fi
echo "Workspace version: $VERSION"

# Library crates in dependency order (a crate appears after everything it
# depends on: normal, build and optional dependencies; dev-dependencies are
# path-only and stripped on publish). `cersei` is the umbrella facade and goes
# last.
CRATES=(
  cersei-types
  cersei-compression
  cersei-embeddings
  cersei-lsp
  bricks-semantic
  cersei-tools-derive
  cersei-hooks
  cersei-mcp
  cersei-provider
  cersei-web
  cersei-skills
  cersei-memory
  cersei-vms
  cersei-tools
  cersei-agentlang
  cersei-agent
  cersei-agentrl
  cersei-workflows
  cersei-tbench
  cersei
)

# The list must match the workspace graph before anything is published.
if ! python3 scripts/check_publish_order.py "${CRATES[@]}"; then
  echo "the crate list above is not publishable as is; nothing was done" >&2
  exit 1
fi

# Return 0 if <name>@<version> already exists on crates.io.
already_published() {
  local name="$1" version="$2" code
  code="$(curl -fsS -o /dev/null -w '%{http_code}' \
    -H 'User-Agent: cersei-sync (https://github.com/pacifio/cersei)' \
    "https://crates.io/api/v1/crates/${name}/${version}" 2>/dev/null || true)"
  [[ "$code" == "200" ]]
}

# Verification stays on by default so a broken crate is caught before upload.
PUBLISH_FLAGS=()
[[ "$ALLOW_DIRTY" == "1" ]] && PUBLISH_FLAGS+=(--allow-dirty)

published_any=0
for crate in "${CRATES[@]}"; do
  if [[ "$DRY_RUN" == "1" ]]; then
    echo "==> [dry-run] $crate@$VERSION"
    cargo publish -p "$crate" --dry-run ${PUBLISH_FLAGS[@]+"${PUBLISH_FLAGS[@]}"}
    continue
  fi

  if already_published "$crate" "$VERSION"; then
    echo "==> $crate@$VERSION already on crates.io — skipping"
    continue
  fi

  echo "==> publishing $crate@$VERSION"
  cargo publish -p "$crate" ${PUBLISH_FLAGS[@]+"${PUBLISH_FLAGS[@]}"}
  published_any=1

  # Don't sleep after the final crate (bash 3.2 has no negative indices).
  if [[ "$crate" != "${CRATES[$((${#CRATES[@]} - 1))]}" ]]; then
    echo "    waiting ${PROPAGATION_SLEEP}s for index propagation..."
    sleep "$PROPAGATION_SLEEP"
  fi
done

if [[ "$DRY_RUN" == "1" ]]; then
  echo "Dry run complete."
elif [[ "$published_any" == "0" ]]; then
  echo "Nothing to do — every crate is already at $VERSION on crates.io."
else
  echo "Done — published crates at $VERSION."
fi
