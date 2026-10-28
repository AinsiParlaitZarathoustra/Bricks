# Tools

This page is a summary. The reference is [Built-in Tools](/docs/built-in-tools) and [Tool API](/docs/api-tools) on the documentation site.

## Built-in tool sets

Each set is a function of `bricks::tools`:

| Function | Contents |
|---|---|
| `all()` | `coding()` and `planning()`: 16 tools |
| `coding()` | `filesystem()`, `shell_with_jobs()` on a fresh registry, and `web()` |
| `filesystem()` | `Read`, `ViewImage`, `Write`, `Edit`, `ApplyPatch`, `Glob`, `Grep`, `CodeSearch`, `CodeScout`, `CodeStats` |
| `shell()` | `Bash` (only commands that end in the call), `PowerShell` |
| `shell_with_jobs(registry)` | `Bash`, `Job` and `PowerShell` on one job registry |
| `web()` | `WebFetch`, `WebSearch` |
| `planning()` | `TodoWrite` |
| `none()` | No tools |

The `bricks` command-line tool and the desktop app start a session with `coding()`. They add `TodoWrite` (the conversation's plan) on their own, and the native sub-agent tools (`Agent`, `Agents`, `AgentControl`, `AgentProfiles`, `SendMessage`) when `[agents] enabled` is set in `bricks.toml`. The agent builder does not add them by itself. The vm tools of `bricks-tools` (feature `vms`) are not in any set: add them yourself.

## Permission levels

Each tool declares a level: `None`, `ReadOnly`, `Write`, `Execute`, `Dangerous`, `Forbidden`. A call can have another level for its input (`permission_level_for`). The level gives the call its class (read, write, execute or network), and the session's permission policy decides on it: ASK, EXECUTE or CUSTOM. The calls the policy asks about go to the approver (`AllowAll`, `DenyAll`, `InteractivePolicy`). See [Permissions](08-permissions.md).

## Custom tools

Implement the `Tool` trait (`name`, `description`, `input_schema`, `execute`), or derive one with `#[derive(Tool)]`. Set `permission_level` and `category` so the policy and the tool listing treat it correctly.
