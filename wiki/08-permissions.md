# Permissions

This page is a summary. The reference is [Tool API, Permission Levels](/docs/api-tools#permission-levels) on the documentation site, and `docs/cli.md#permissions` in the repository for the frontends.

- One engine decides every tool call. It reads the session's policy: a profile and, for the CUSTOM profile, its rules. It returns `allow`, `ask` or `deny`, with the reason and the policy revision.
- Each call has a class: **read** (reads and searches), **write** (file changes), **execute** (commands and sub-agents) or **network** (the web tools). A tool's permission level gives its class; a `Forbidden` tool is refused in every profile.
- Profiles: **ASK** runs reads and asks about the rest; **EXECUTE** runs everything; **CUSTOM** gives an effect per class and per tool (a tool's own rule wins), and is the only profile that refuses calls. A policy never chosen behaves as ASK.
- A call the engine says to ask about goes to the **approver**, a `PermissionPolicy`: `AllowAll`, `DenyAll`, `InteractivePolicy` (a callback, or `InteractivePolicy::via_stream()`). Its answer covers that one call. There is no allow for the session.
- Builder: `.permission_policy(approver)` sets the approver (alone, ASK); `.permission_profile(SessionPolicy)` sets the policy (without it: EXECUTE with no approver, ASK with one); `.permission_cell(..)` shares a policy within a session.
- Sub-agents share their parent's policy. A policy change bumps a revision: a request already waiting is not answered by it; later calls follow the new revision.
- Storage is outside `bricks.toml`: the default for new sessions is in `~/.bricks/permissions.json`, and each session keeps its own `permissions.json`. The `[permissions]` section of `bricks.toml` is no longer read.
