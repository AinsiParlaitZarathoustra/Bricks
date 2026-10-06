# Bricks shell driver (bash ≥ 3.2). Runs at the top level of the persistent
# shell, so commands share one scope: variables, `cd`, aliases, functions and
# options persist from one request to the next.
#
# Channels (installed by Bricks before exec):
#   fd 198  requests  — NUL-terminated fields: id, kind, script, out, err, in
#   fd 199  replies   — NUL-terminated fields: READY pid version |
#                       START id | END id status cwd
# The command's own stdin/stdout/stderr are the files named in the request;
# fds 198 and 199 are closed for the command, so `read`, `cat` or a child
# process can never consume or forge a control message.
#
# Names starting with `__bricks_` are reserved.
shopt -s expand_aliases
__bricks_reply() { builtin printf '%s\0' "$@" >&199; }
__bricks_reply READY "$$" "$BASH_VERSION"
while :; do
  IFS= builtin read -r -d '' -u 198 __bricks_id || break
  IFS= builtin read -r -d '' -u 198 __bricks_kind || break
  IFS= builtin read -r -d '' -u 198 __bricks_script || break
  IFS= builtin read -r -d '' -u 198 __bricks_out || break
  IFS= builtin read -r -d '' -u 198 __bricks_err || break
  IFS= builtin read -r -d '' -u 198 __bricks_in || break
  case $__bricks_kind in
  run)
    __bricks_reply START "$__bricks_id"
    { builtin . "$__bricks_script"; } >"$__bricks_out" 2>"$__bricks_err" <"$__bricks_in" 198<&- 199>&-
    __bricks_reply END "$__bricks_id" "$?" "$PWD"
    ;;
  snapshot)
    {
      builtin printf 'builtin cd -- %q 2>/dev/null\n' "$PWD"
      builtin shopt -p
      builtin set +o
      builtin declare -px
      builtin declare -f
      builtin alias -p
    } >"$__bricks_out" 2>/dev/null 198<&- 199>&-
    __bricks_reply END "$__bricks_id" "$?" "$PWD"
    ;;
  describe)
    # Definitions of the session's aliases/functions among the given names.
    for __bricks_n in $__bricks_script; do
      builtin alias -- "$__bricks_n" 2>/dev/null
      builtin declare -f -- "$__bricks_n" 2>/dev/null
    done >"$__bricks_out" 198<&- 199>&-
    __bricks_reply END "$__bricks_id" 0 "$PWD"
    ;;
  *)
    __bricks_reply END "$__bricks_id" 254 "$PWD"
    ;;
  esac
done
