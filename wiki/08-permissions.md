# Permissions

This page is a summary. The full reference is [Tool API, Permission Levels](/docs/api-tools#permission-levels) on the documentation site, and `docs/cli.md#permissions` in the repository for the CLI and desktop app.

- One permission engine checks every tool call against the session's policy (a profile plus optional custom rules). It returns `allow`, `ask`, or `deny`, along with a reason and the policy revision.
- Each call maps to a class: **read** (reads and searches), **write** (file edits), **execute** (commands and sub-agents), or **network** (web tools). A tool's permission level sets its class, and a `Forbidden` tool is always denied.
- Profiles: **ASK** allows reads and asks about everything else; **EXECUTE** allows everything; **CUSTOM** sets an effect per class and per tool (tool rules take priority) and is the only profile that can deny calls. When no policy is set, sessions default to ASK.
- Any call marked `ask` goes to the **approver** (`PermissionPolicy`: `AllowAll`, `DenyAll`, or `InteractivePolicy`, including `InteractivePolicy::via_stream()`). Each approval applies to that single call only.
- On `AgentBuilder`: `.permission_policy(approver)` sets the approver, `.permission_profile(SessionPolicy)` sets the policy, and `.permission_cell(...)` shares a policy within a session.
- Sub-agents share their parent's policy. Changing the policy bumps its revision so pending requests are not answered by the new rules.
- Policy storage lives outside `bricks.toml`: the default for new sessions is `~/.bricks/permissions.json` (or `$BRICKS_HOME/permissions.json`), and each session keeps its own `permissions.json`.
