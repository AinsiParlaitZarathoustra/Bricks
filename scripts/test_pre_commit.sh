#!/usr/bin/env bash
#
# test_pre_commit.sh — the optional pre-commit hook, in temporary
# repositories: it judges the staged content (not the working tree), finds
# bad formatting, whitespace errors and synthetic secrets without printing
# them, handles deletions, renames and names with spaces or Unicode, ignores
# an unstaged allowlist, modifies nothing, and the installer keeps an
# existing hook. Nothing is installed in this checkout. Runs with bash 3.2+.

set -euo pipefail
cd "$(dirname "$0")/.."
src="$PWD"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/bricks-hook-test.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT
fail() { echo "FAIL: $*" >&2; exit 1; }
before_hooks="$(ls -A .git/hooks 2>/dev/null | grep -v '\.sample$' || true)"

n=0
new_repo() {
  n=$((n + 1))
  r="$TMP/repo$n"
  mkdir -p "$r/scripts/hooks" "$r/src"
  cp "$src/scripts/check_secrets.py" "$r/scripts/"
  cp "$src/scripts/hooks/pre-commit" "$src/scripts/hooks/install.sh" "$r/scripts/hooks/"
  : > "$r/scripts/secret-allowlist.txt"
  printf '[package]\nname = "t"\nversion = "0.1.0"\nedition = "2021"\n' > "$r/Cargo.toml"
  printf 'mod util;\n\nfn main() {\n    util::hi();\n}\n' > "$r/src/main.rs"
  printf 'pub fn hi() {}\n' > "$r/src/util.rs"
  git -C "$r" init -q -b main
  git -C "$r" config user.email t@t
  git -C "$r" config user.name t
  git -C "$r" config commit.gpgsign false
  git -C "$r" add -A
  git -C "$r" commit -q -m init
}
# Run the hook as git would (cwd: the repository), capture its output.
hook() { (cd "$r" && bash scripts/hooks/pre-commit) >"$TMP/out" 2>&1; }
state() { (cd "$r" && git ls-files -s && git status --porcelain=v1 -z | od -c | tail -n 3 && find . -path ./.git -prune -o -type f -print0 | sort -z | xargs -0 shasum) ; }

# 1. A correct index is accepted; badly formatted Rust is refused.
new_repo
printf 'pub fn hi() {\n    let x = 1;\n    let _ = x;\n}\n' > "$r/src/util.rs"
git -C "$r" add src/util.rs
hook || fail "correct index refused: $(cat "$TMP/out")"
printf 'pub fn hi(){let x=1;let _=x;}\n' > "$r/src/util.rs"
git -C "$r" add src/util.rs
hook && fail "badly formatted Rust accepted"
grep -q "not formatted" "$TMP/out" || fail "no instruction: $(cat "$TMP/out")"

# 2. Working tree correct, index incorrect: refused (the index decides).
printf 'pub fn hi() {}\n' > "$r/src/util.rs"
hook && fail "judged the working tree instead of the index"
# 3. Index correct, working tree incorrect: accepted.
git -C "$r" add src/util.rs
printf 'pub fn hi(){let y=2;}\n' > "$r/src/util.rs"
hook || fail "judged the working tree: $(cat "$TMP/out")"
# A partially staged file: the staged half is judged.
printf 'pub fn hi() {}\n\npub fn two() {}\n' > "$r/src/util.rs"
git -C "$r" add src/util.rs
printf 'pub fn hi() {}\n\npub fn two() {}\npub fn three(){}\n' > "$r/src/util.rs"
hook || fail "partially staged file judged on unstaged bytes"

# 4. Whitespace errors and conflict markers in the index.
new_repo
printf 'line with trailing space \n' > "$r/notes.txt"
git -C "$r" add notes.txt
hook && fail "whitespace error accepted"
printf '<<<<<<< ours\na\n=======\nb\n>>>>>>> theirs\n' > "$r/notes.txt"
git -C "$r" add notes.txt
hook && fail "conflict markers accepted"

# 5. A synthetic secret is found, never printed; an unstaged allowlist entry
#    does not excuse it; a staged one does.
new_repo
google="AIza$(printf 'Q%.0s' $(seq 1 35))"
printf 'key = "%s"\n' "$google" > "$r/config.txt"
git -C "$r" add config.txt
hook && fail "secret accepted"
grep -q "google-api-key" "$TMP/out" || fail "secret not reported: $(cat "$TMP/out")"
grep -q "$google" "$TMP/out" && fail "the value was printed"
# (The scanner exits 1 on a finding: expected here.)
fp="$( (cd "$r" && python3 scripts/check_secrets.py config.txt 2>/dev/null || true) | sed -n 's/.*(\(value#[0-9a-f]*\)).*/\1/p' | head -n1)"
[ -n "$fp" ] || fail "no fingerprint"
printf 'config.txt\t%s\tsynthetic\n' "$fp" > "$r/scripts/secret-allowlist.txt"
hook && fail "an unstaged allowlist entry excused a staged secret"
git -C "$r" add scripts/secret-allowlist.txt
hook || fail "staged allowlist entry not honoured: $(cat "$TMP/out")"

# 6. Deletion, rename, spaces and Unicode in names.
new_repo
mkdir -p "$r/src/données utiles"
printf 'pub fn été() {}\n' > "$r/src/données utiles/mod é.rs"
git -C "$r" add -A
hook || fail "Unicode/space names: $(cat "$TMP/out")"
git -C "$r" commit -q -m names
git -C "$r" mv "src/données utiles/mod é.rs" "src/données utiles/renommé ✓.rs"
git -C "$r" rm -q src/util.rs
printf 'fn main() {}\n' > "$r/src/main.rs"
git -C "$r" add src/main.rs
hook || fail "rename + deletion: $(cat "$TMP/out")"
printf 'pub fn  bad( ){}\n' > "$r/src/données utiles/renommé ✓.rs"
git -C "$r" add -A
hook && fail "bad formatting in a renamed Unicode file accepted"
# A module missing from the index: reported as such, not as formatting.
new_repo
git -C "$r" rm -q --cached src/util.rs
printf 'mod util;\n\nfn main() {\n    util::hi();\n    util::hi();\n}\n' > "$r/src/main.rs"
git -C "$r" add src/main.rs
hook && fail "a module missing from the index was accepted"
grep -q "could not read the staged Rust code" "$TMP/out" || fail "wrong diagnostic: $(cat "$TMP/out")"
# Non-Rust files are not formatted.
new_repo
printf 'not   rust {{{\n' > "$r/README.md"
git -C "$r" add README.md
hook || fail "a non-Rust file was formatted: $(cat "$TMP/out")"

# 7. Nothing is modified: index, working tree, files.
new_repo
printf 'pub fn hi(){}\n' > "$r/src/util.rs"
git -C "$r" add src/util.rs
printf 'pub fn hi( ) {}\n' > "$r/src/util.rs"
s1="$(state)"
hook || true
s2="$(state)"
[ "$s1" = "$s2" ] || fail "the hook changed the repository"
# A failing tool propagates: rustfmt missing → explicit error, no success.
mkdir -p "$TMP/bin"
for t in git python3 sed head mktemp cp rm cat dirname env bash; do
  p="$(command -v "$t")" && ln -sf "$p" "$TMP/bin/$t"
done
( cd "$r" && PATH="$TMP/bin" bash scripts/hooks/pre-commit ) >"$TMP/out" 2>&1 && fail "succeeded without rustfmt"
grep -q "rustfmt is required" "$TMP/out" || fail "no diagnostic: $(cat "$TMP/out")"

# 8. The installer: explicit only, keeps an existing hook, no hooksPath.
new_repo
hooks="$r/.git/hooks"
[ ! -e "$hooks/pre-commit" ] || fail "installed without being asked"
printf '#!/bin/sh\necho mine\n' > "$hooks/pre-commit"
(cd "$r" && bash scripts/hooks/install.sh enable) >/dev/null 2>&1 && fail "replaced an existing hook"
grep -q "echo mine" "$hooks/pre-commit" || fail "existing hook changed"
(cd "$r" && bash scripts/hooks/install.sh disable) >/dev/null 2>&1 && fail "removed a hook that is not ours"
rm "$hooks/pre-commit"
(cd "$r" && bash scripts/hooks/install.sh enable) >/dev/null || fail "enable failed"
[ -L "$hooks/pre-commit" ] || fail "not linked"
[ -z "$(git -C "$r" config --get core.hooksPath || true)" ] || fail "core.hooksPath changed"
# Through a real commit: refused when the index is bad, accepted after.
printf 'pub fn hi(){}\n' > "$r/src/util.rs"
git -C "$r" add src/util.rs
git -C "$r" commit -q -m bad >/dev/null 2>&1 && fail "commit with bad formatting went through"
printf 'pub fn hi() {}\n\npub fn more() {}\n' > "$r/src/util.rs"
git -C "$r" add src/util.rs
git -C "$r" commit -q -m good || fail "a correct commit was refused"
(cd "$r" && bash scripts/hooks/install.sh disable) >/dev/null || fail "disable failed"
[ ! -e "$hooks/pre-commit" ] || fail "not disabled"
git -C "$r" config core.hooksPath custom-hooks
(cd "$r" && bash scripts/hooks/install.sh enable) >/dev/null 2>&1 && fail "ignored core.hooksPath"
[ "$(git -C "$r" config --get core.hooksPath)" = custom-hooks ] || fail "core.hooksPath changed"

# This checkout: still no hook installed by the tests.
after_hooks="$(ls -A .git/hooks 2>/dev/null | grep -v '\.sample$' || true)"
[ "$before_hooks" = "$after_hooks" ] || fail "this checkout's hooks changed"
echo "pre-commit hook: all checks passed"
