# Tools

This page is a summary. The full reference is [Built-in Tools](/docs/built-in-tools) and [Tool API](/docs/api-tools) on the documentation site.

## Built-in tool sets

Each set is a function in `bricks::tools`:

| Function | Contents |
|---|---|
| `all()` | `coding()` plus `planning()`: 14 tools (13 when `PowerShell` is unavailable) |
| `coding()` | `filesystem()`, `shell_with_jobs(...)` on a fresh registry, and `web()` |
| `filesystem()` | `Read` (text and images), `Write`, `Edit` (single and multi-edit), `ApplyPatch`, `Glob`, `Grep`, `CodeScout`, `CodeStats` |
| `shell()` | `Bash` (commands that finish in the call) and `PowerShell` (when available) |
| `shell_with_jobs(registry)` | `Bash`, `Job`, and `PowerShell` (when available) on one job registry |
| `web()` | `WebFetch`, `WebSearch` |
| `planning()` | `TodoWrite` |
| `none()` | No tools |

`CodeSearch` is not in `filesystem()`: add `CodeSearchTool::with_embeddings(...)` when an embedding provider is configured.

The `bricks` CLI and the desktop app start a session with `coding()` and add `TodoWrite` on their own. They also add the memory tools (`memory_search`, `memory_write`) when long-term memory is enabled, and the sub-agent tools (`Agent`, `Agents`, `AgentControl`, `AgentProfiles`, `SendMessage`) when `[agents] enabled = true` is set in `bricks.toml`. `Agent::builder()` does not add those automatically. VM tools from `bricks-tools` (feature `vms`) are also added separately.

## Permission levels

Each tool declares a level: `None`, `ReadOnly`, `Write`, `Execute`, `Dangerous`, or `Forbidden`. A tool can also pick a level per call (`permission_level_for`). The level maps to a class (read, write, execute, or network), and the session's policy decides whether to allow, ask, or deny it. Calls that need approval go to the `PermissionPolicy` approver (`AllowAll`, `DenyAll`, or `InteractivePolicy`). See [Permissions](08-permissions.md).

## Custom tools

Implement the `Tool` trait (`name`, `description`, `input_schema`, `execute`), or derive it with `#[derive(Tool)]`. Set `permission_level` and `category` so permission checks and tool listings handle your tool properly.
