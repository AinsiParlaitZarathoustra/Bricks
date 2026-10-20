# Tools

This page is a summary. The reference is [Built-in Tools](/docs/built-in-tools) and [Tool API](/docs/api-tools) on the documentation site.

## Built-in tool sets

Each set is a function of `bricks::tools`:

| Function | Contents |
|---|---|
| `all()` | Every built-in tool, plus `ToolSearch` |
| `coding()` | `filesystem()`, `shell()` and `web()` |
| `filesystem()` | `Read`, `Write`, `Edit`, `ApplyPatch`, `Glob`, `Grep`, `CodeSearch`, `CodeScout`, `NotebookEdit` |
| `shell()` | `Bash` with its background-task tools, `Job`, `PowerShell` |
| `web()` | `WebFetch`, `WebSearch` |
| `planning()` | `EnterPlanMode`, `ExitPlanMode`, `TodoWrite` |
| `scheduling()` | the `Cron` tools, `Sleep`, `RemoteTrigger` |
| `orchestration()` | `SendMessage`, the `Task*` tools, `EnterWorktree`, `ExitWorktree` |
| `none()` | No tools |

The `bricks` command-line tool and the desktop app start a session with `coding()`. They add the native sub-agent tools (`Agent`, `Agents`, `AgentControl`, `AgentProfiles`) when `[agents] enabled` is set in `bricks.toml`. The agent builder does not add them by itself. The vm tools of `bricks-tools` (feature `vms`) are not in any set: add them yourself.

## Permission levels

Each tool declares a level, and the permission policy decides on each call: `None`, `ReadOnly`, `Write`, `Execute`, `Dangerous`, `Forbidden`. A policy can differ per call through `permission_level_for`. The built-in policies include `AllowAll`, `AllowReadOnly`, `DenyAll`, `RuleBased` and `InteractivePolicy`.

## Custom tools

Implement the `Tool` trait (`name`, `description`, `input_schema`, `execute`), or derive one with `#[derive(Tool)]`. Set `permission_level` and `category` so the policy and the tool listing treat it correctly.
