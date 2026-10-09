#!/usr/bin/env bash
#
# test_sync_cargo.sh — check sync_cargo.sh without publishing anything.
#
# A fake `cargo` (metadata goes to the real one; every other call is only
# logged) and a fake `curl` (crates.io answers 200 for the crates named in
# $ALREADY, 404 otherwise) are put first in PATH. Checks: the order of the
# publish calls, the skip of crates already published, no `cargo publish`
# without --dry-run in dry-run mode, no network call at all, and the refusal
# of an incoherent list. Runs with bash 3.2 (macOS) and later.

set -euo pipefail
cd "$(dirname "$0")/.."

REAL_CARGO="$(command -v cargo)"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
LOG="$TMP/calls.log"

cat >"$TMP/cargo" <<EOF
#!/usr/bin/env bash
if [ "\$1" = "metadata" ]; then exec "$REAL_CARGO" "\$@"; fi
echo "cargo \$*" >>"$LOG"
EOF
cat >"$TMP/curl" <<'EOF'
#!/usr/bin/env bash
url="${@: -1}"
echo "curl $url" >>"$LOG"
name="$(echo "$url" | sed -E 's#.*/crates/([^/]+)/.*#\1#')"
case " ${ALREADY:-} " in *" $name "*) printf 200 ;; *) printf 404 ;; esac
EOF
cat >"$TMP/sleep" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
chmod +x "$TMP/cargo" "$TMP/curl" "$TMP/sleep"

fail() { echo "FAIL: $*" >&2; exit 1; }
expected="$(sed -n '/^CRATES=(/,/^)/p' sync_cargo.sh | grep -v 'CRATES=(\|^)' | tr -d ' ')"

# 1. Dry run: one `cargo publish --dry-run` per crate, in order; no curl.
: >"$LOG"
PATH="$TMP:$PATH" "$BASH" ./sync_cargo.sh --dry-run >/dev/null
got="$(sed -n 's/^cargo publish -p \([^ ]*\) --dry-run$/\1/p' "$LOG")"
[ "$got" = "$expected" ] || fail "dry-run order: $got"
grep -q '^curl' "$LOG" && fail "dry-run reached the network"
grep '^cargo publish' "$LOG" | grep -qv -- '--dry-run' && fail "dry-run published"

# 2. Publication: crates already on crates.io are skipped, the others are
#    published in order, nothing else.
: >"$LOG"
ALREADY="cersei-types cersei-lsp" PATH="$TMP:$PATH" "$BASH" ./sync_cargo.sh >/dev/null
got="$(sed -n 's/^cargo publish -p \([^ ]*\)$/\1/p' "$LOG")"
want="$(echo "$expected" | grep -vx 'cersei-types' | grep -vx 'cersei-lsp')"
[ "$got" = "$want" ] || fail "publish order or skip: $got"

# 3. --allow-dirty is passed on.
: >"$LOG"
PATH="$TMP:$PATH" "$BASH" ./sync_cargo.sh --dry-run --allow-dirty >/dev/null
grep -q -- '--dry-run --allow-dirty' "$LOG" || fail "--allow-dirty not passed"

# 4. The checker refuses an incoherent list.
for bad in \
  "cersei-tools cersei-types" \
  "cersei-types cersei-agent" \
  "cersei-types no-such-crate" \
  "cersei-types cersei-testkit" \
  "cersei-types cersei-types"; do
  # shellcheck disable=SC2086
  if python3 scripts/check_publish_order.py $bad 2>/dev/null; then
    fail "accepted: $bad"
  fi
done

echo "sync_cargo.sh: all checks passed"
