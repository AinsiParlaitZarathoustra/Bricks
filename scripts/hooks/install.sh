#!/usr/bin/env bash
#
# install.sh — enable, disable or inspect the optional Bricks pre-commit hook
# in this clone. Nothing happens unless asked: Bricks never installs it.
#
#   scripts/hooks/install.sh status
#   scripts/hooks/install.sh enable     # links .git/hooks/pre-commit to scripts/hooks/pre-commit
#   scripts/hooks/install.sh disable    # removes that link (only if it is ours)
#
# A hook that is already there (yours, another tool's) is never replaced or
# removed; `core.hooksPath` is never changed. When it is set, hooks live
# elsewhere: the script says so and lets you add the hook there yourself.

set -euo pipefail

root="$(git rev-parse --show-toplevel)"
cd "$root"
ours="$root/scripts/hooks/pre-commit"
hooks_path="$(git config --get core.hooksPath || true)"
target="$(git rev-parse --git-path hooks)/pre-commit"
case "$target" in /*) ;; *) target="$root/$target" ;; esac

is_ours() {
  [ -L "$target" ] && [ "$(readlink "$target")" = "$ours" ]
}

cmd="${1:-status}"
if [ -n "$hooks_path" ] && [ "$cmd" != status ]; then
  echo "core.hooksPath is set ($hooks_path): your hooks are managed there." >&2
  echo "Nothing changed. To use the Bricks hook, call $ours from your pre-commit hook." >&2
  exit 2
fi

case "$cmd" in
  status)
    if [ -n "$hooks_path" ]; then
      echo "core.hooksPath = $hooks_path (hooks managed outside .git/hooks)"
    elif is_ours; then
      echo "enabled: $target -> $ours"
    elif [ -e "$target" ] || [ -L "$target" ]; then
      echo "another pre-commit hook is installed: $target (not Bricks')"
    else
      echo "not enabled"
    fi
    ;;
  enable)
    if is_ours; then
      echo "already enabled"
    elif [ -e "$target" ] || [ -L "$target" ]; then
      echo "a pre-commit hook already exists: $target" >&2
      echo "Nothing changed. Keep yours and call $ours from it, or move it away first." >&2
      exit 2
    else
      mkdir -p "$(dirname "$target")"
      ln -s "$ours" "$target"
      echo "enabled: $target -> $ours (bypass once with git commit --no-verify)"
    fi
    ;;
  disable)
    if is_ours; then
      rm "$target"
      echo "disabled"
    elif [ -e "$target" ] || [ -L "$target" ]; then
      echo "the pre-commit hook is not Bricks' ($target): left as is" >&2
      exit 2
    else
      echo "not enabled"
    fi
    ;;
  *)
    echo "usage: $0 [status|enable|disable]" >&2
    exit 2
    ;;
esac
