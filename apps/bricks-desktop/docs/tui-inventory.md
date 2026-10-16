# TUI inventory and desktop mapping (Sprint 14.1)

Status: 2026-10-16, branch `Bricks_Desktop`. Owner: Haiku 1.
Sources read: `crates/bricks-tui/src/{commands,inspect,diag,ui,state,lib,overlay,composer,view,statusbar,term}.rs`
and `crates/bricks-cli/src/{args,main,tui,sessions}.rs`. The `… 2.rs` copies are user data and were ignored.

The slash registry now lives in `cersei_agent::control::slash` (names, aliases, arguments, English and French
descriptions, errors in both languages). `crates/bricks-tui/src/commands.rs` re-exports it. The desktop maps it
in `apps/bricks-desktop/src-tauri/src/commands/slash.rs`. Nothing is duplicated.

Legend: **Engine** = `Command` sent to the engine (`session_command` on the desktop). **Present** = the
interface opens its own view. **Local** = handled in the interface only (no engine call).
Event names are the engine's `Event` tags (snake_case).

---

## 1. Slash commands

| Command (alias) | Arguments | Kind | What the TUI does | Desktop access (proposal) | Engine command / API → answer | Check |
|---|---|---|---|---|---|---|
| `/model` (`/m`) | `[provider/model [profile]]` | Present, or Engine | No argument: model picker. With arguments: `SetModel{model, reasoning}`. | Model picker (`ui.openPicker('model')`) and typed form | `SetModel` → `model_changed` | Picker marks the current model; refusal shown |
| `/effort` | `[profile id \| default]` | Present, or Engine | No argument: effort picker. `default` sends `SetReasoning{profile: null}`, an id sends `SetReasoning{profile}`. | Effort picker (`ui.openPicker('effort')`) | `SetReasoning` → `reasoning_changed` | `/effort default` resets; unknown id refused |
| `/search` (`/s`) | `<text> \| re:<regex>` | Engine | Workspace text or regex search (never starts a language server). Result is a cell. | Palette → search view (not conversation search) | `Search{text, regex}` → `search_results` (hits, omitted, notes, elapsed) | Empty text refused; regex error shown |
| `/agents` | `[words] \| reload` | Engine | Lists sub-agent profiles (20 per page) or reloads them. | Catalogue view (`features/catalog/AgentProfilesView`) | `ListAgentProfiles{query, page}` / `ReloadAgentProfiles` → `agent_profiles` | Search and reload shown; diagnostics kept |
| `/agents running` | | Engine | Lists sub-agent instances. | Instances list (`AgentInstancesView`) | `AgentControl{action: list}` → `agent_control_result` | Instances from this session only |
| `/agents status\|result\|cancel <id>` | | Engine | Status, result or cancel of one instance. | Inspector (`AgentInspector`) with cancel button | `AgentControl{action, agent_id}` → `agent_control_result` | Cancel really stops the agent |
| `/skills` | `[words] [page n] \| reload \| trust \| deny` | Engine | Catalogue (20 per page), reload, trust or deny this project's skills for the session. | Skills view (`SkillsView`) | `ListSkills{query, page}`, `ReloadSkills`, `TrustProjectSkills{allow}` → `skills`, `skills_trust_requested`, `skills_trust_resolved` | No automatic trust |
| `/skill <name> <request>` | | Engine | Activates the skill, then sends the request. Nothing is sent if activation fails. | Composer command with skill chip | `Submit{prompt: {blocks, skills: [name]}}` → `skill_activated` or refusal (`command_rejected`, `skill_activation_failed`) | Refused skill: request not sent |
| `/changes <id> [inspect\|apply\|discard]` | default `inspect` | Engine | Shows the patch, applies it to the working tree (a conflict writes nothing), or discards it. | ChangeSet view (`ChangesetView`), with the existing confirmations | `AgentControl{inspect_changes \| apply_changes \| discard_changes, changeset_id}` → `agent_control_result`, `changes_updated` | Conflict: nothing written |
| `/jobs` | | Engine | Background jobs of the session (engine jobs, not user terminals). | Jobs view (`JobsView`) | `AgentControl{action: jobs}` → `job_started`, `job_output`, `job_finished` | Distinct from PTY terminals |
| `/jobs stop <id>` | | Engine | Graceful stop, then forced. | Stop button in `JobsView` | `AgentControl{action: stop_job, job_id}` → `job_finished` | Stopped job shows its end |
| `/memory` | | Present | Memory space, last recall, maintenance state (see §2). | Memory view (`features/inspect/MemoryView`) | No command. Snapshot `memory_space`; events `memory_recalled`, `memory_maintenance_started`, `memory_maintenance_finished` | Shows "none" when memory is disabled |
| `/context` (`/ctx`) | `[details]` | Present | Context occupation and availability. `details`: measurement, margins, notes. | Context ring + view (`ContextView`) | Snapshot `context` (ContextStatus); event `context` | Estimates marked `≈` |
| `/cost` (`/usage`) | `[details]` | Present | Session usage and cost. `details`: one row per response. | Cost view (`CostView`) | Snapshot `usage` and `context.totals`; events `usage`, `run_usage` | Unknown price stays "Not priced" |
| `/session` | | Present | Id, title, folder, model and effort, activity, memory space. | Session info (`SessionView`) | Snapshot (or `session_snapshot`) | |
| `/resume` (`/sessions`) | `[session id]` | Present (no id), Engine→Present (id) | No id: session picker (this project; Tab: all projects). With id: resume. | Picker (`SessionsView`, `sessions_list` / `sessions_search`). With id: `SlashPresent::Resume` → `app.resumeSession` (`session_open`), never `Command::Resume` | `session_open` (desktop path); picker from `sessions_list`, `sessions_search` | Opens in its own project and folder |
| `/compact` | | Engine | Summarises the context now (refused during a run). Ctrl+C cancels a pending one. | Action with pending state | `Compact` → `compaction`, or `command_rejected` | Refusal during a run shown |
| `/clear` | | Engine | Empties the active context; history kept in the session's raw history. | Action with explanation and confirmation (spec §5) | `ClearContext` → `context_cleared{messages_removed}` | Conversation is not deleted |
| `/diff` | | Present | Three sections: proposed (pending approvals with previews), applied in this session, git diff of the working tree. | Changes panel (`change_log_list`, `change_log_diff`), approvals card, `git_diff` | Pending approvals (`approval_requested`), events `edit_applied`, git (`git_diff`) | Shell and MCP effects are not file diffs |
| `/tools` | | Present | Tools available to the agent: name, permission level, first line of description. | Tools view (`ToolsView`) | `session_tools` (adapter over `Controller::tools()`) | No secrets |
| `/mcp` | | Present | MCP servers and their state. | MCP view (`McpView`) | `session_mcp` (adapter over `Controller::mcp_statuses()`) | No install or manage screen |
| `/config` | | Present | Effective configuration, approval policy, allowed for the session, no keys or auth headers. | Config view (`ConfigView`) | `session_config` (adapter over snapshot, `approval_rules`, `allowed_for_session`) | Keys never shown |
| `/file <path>` | | Local | Attaches a file (captured at send). Checks the file exists, relative to the working folder. | Composer attachment (chip) | Prompt block `file` at `Submit` | Paths with spaces and Unicode kept |
| `/folder <path>` | | Local | Attaches a bounded folder listing. | Composer attachment | Prompt block `folder` | Folder must exist |
| `/image <path>` (`/img`) | | Local | Attaches an image (png, jpeg, gif, webp). | Composer attachment | Prompt block `image` | Model must accept images |
| `/help` (`/?`) | | Present | Keys and commands from the registry. | Help view (`HelpView`) from `slash_commands` plus desktop keys | `slash_commands` | Generated from the registry |
| `/quit` (`/exit`, `/q`) | | Present | Leaves; the session stays stored. | Native safe quit | `app_quit` (cancels runs, saves, stops PTYs) | No abrupt stop of other sessions |

Unknown command or bad argument: the TUI keeps the draft and shows the English message. The desktop
keeps the draft and shows `SlashError::message_fr()` as `IpcErrorCode::Invalid`.

---

## 2. Inspectors: what each shows and where it comes from

| Inspector (title) | Fields | Source |
|---|---|---|
| session | id, title (`(none yet)`), working folder, model (+ reasoning), activity, memory space, resume hint | Snapshot |
| context | model, window (or "Not configured"), prompt budget, current usage (`≈` if estimated), occupation %, available (safe), provenance, session total, requests | Snapshot `context` |
| context details | provenance, measured or counted, estimated, estimate method, upper bound; window, max input, max output, output reserve, prompt budget, safety margin; safe and raw availability; occupation; session cumulative (input, cache read and write, output, reasoning, requests, compaction requests, without usage); notes | Snapshot `context` |
| cost | current model (+ effort), requests (+ compaction), without usage, pricing state (estimate, partial, not priced), input, cache read and write, output, reasoning; partial: known subtotal and unpriced categories | Snapshot `usage`, `context.totals` |
| cost details | per response: input, cache read and write, output, reasoning, cost (`partial ≥` when partial); only responses this interface received | `app.status.turns_usage` (events), total from snapshot |
| memory | space (or "enable [memory] in bricks.toml"), last recall (items, tokens of budget, omitted), maintenance (running, or last outcome with extracted, created, updated, embedded, failed, pending, errors) | Snapshot `memory_space`; events `memory_recalled`, `memory_maintenance_*` |
| config | working folder, model (+ reasoning), approvals asked interactively, allowed for session, approval policy (JSON) | Snapshot |
| tools | name, level, first line of description | `Controller::tools()` |
| mcp | `name: status`, or "No MCP server is configured" | `Controller::mcp_statuses()` |
| diff | proposed changes per pending approval with per-file diff, applied changes (tool, path, kind, counts), `git diff --stat --patch` (3000 lines max) | `app.pending` (previews), `app.applied` (events `edit_applied`), git |
| help | key list, then command list (name, arguments, aliases, description) | Registry |
| details (Ctrl+O) | last tool calls: name, summary, status, duration, output head (256 KB kept), extra bytes; last reasoning | Live cells |
| sessions picker | label (● current), time, message count, model, folder ("folder no longer found" when missing); Tab: all projects | `list_sessions_in(Workspace)`, `sessions()` |
| model picker | ● current, selection, name, max input tokens, profiles, "Not priced" | `Controller::models()` (`ModelChoice`) |
| effort picker | "Default" (the model's declared default), profiles by label, ● requested, title shows effective profile | `models()`, snapshot `reasoning`, `effective_reasoning` |
| profile picker | "(default)" then profiles of one model | `models()` |
| approval details (`d`, Enter) | tool and level, description, per-file diff or input JSON with "No file preview" note | `ApprovalRequest` (`approval_requested`) |
| search result (cell) | query, status, hits, omitted, notes, elapsed | `search_results` |

---

## 3. Interactions, live area and state

| Interaction | Behaviour in the TUI | Desktop proposal |
|---|---|---|
| Send | Enter sends. Shift+Enter, Alt+Enter, Ctrl+J: newline. A line ending in `\` then Enter: newline. | Composer: Enter sends, Shift+Enter newline |
| Paste | Inserted, never sent | Native paste |
| `@` | File index popup: Tab or Enter attaches (folder, image or file by extension), Ctrl+R refreshes, Esc closes | Attachment picker; no contract command for the file index yet (gap, §5) |
| `/` | Command popup: Tab completes, Enter runs when there are no arguments, Esc closes | Command palette (`slash_commands`) |
| Prompt history | ↑/↓ on first or last line. In memory for this process only (not reloaded from stored sessions). Draft kept while browsing. | Composer history kept per window; draft never overwritten |
| Editing keys | Ctrl+W delete word, Ctrl+U clear, Ctrl+A and Ctrl+E home and end, Ctrl+Left/Right word moves, Shift selection, Backspace on empty input removes the last attachment | Native text field |
| Submit while running | Refused with a message; text kept | Same: pending state, text kept |
| Cancel | Ctrl+C cancels the run, the memory maintenance, or a pending `/compact` (`Command::Cancel`). Idle: closes an overlay, else clears the composer, twice within 1.5 s quits. | Stop button → `session_command` `Cancel` |
| Quit | Ctrl+D on empty input | `app_quit` |
| Approval focus | Tab focuses (automatic when a request arrives). `y` allow, `a` allow for the session, `n` reject (reason "rejected in the interface"), `d` or Enter details, Tab or Esc leaves focus | Approval card: allow, session, reject, details (`Command::Approve`) |
| Reasoning | Ctrl+T shows or hides live reasoning (only what the provider exposes) | Collapsible reasoning block |
| Details | Ctrl+O: last tool calls and reasoning | Tool call details (outputs from `tool_finished`) |
| Text overlay | ↑↓, PgUp/PgDn, Home/End scroll; Esc or q closes | Dialog |
| Picker | Typing filters; Backspace; ↑↓; Enter chooses; Tab switches views (sessions); Esc closes | Select or palette |
| Message line | Transient messages (e.g. "cancelling…", "Ctrl+C again to quit") | Toast |
| Live cells | Streaming text, reasoning, tool calls with progress and duration, approvals, edits, notices, errors, sub-agents, search results, run end | Conversation components |
| Status bar | Model and effort, `Ctx` % (`≈` if estimated), cost ("Not priced" when partial), activity (spinner, elapsed, Ctrl+C), memory update, project; `+N −N` edits this session | `ContextRing`, `DiffStat`, topbar |
| Scrollback | Finished cells written to the terminal scrollback | Transcript (no terminal equivalent needed) |
| Resume | Session opened: folder changes to the session's folder; history reloaded as cells (`[system]` user messages hidden) | Desktop open path |
| Links | `--hyperlinks` (OSC 8) | Native links (sanitised) |

---

## 4. CLI routes

| Invocation | Starts the TUI | Notes |
|---|---|---|
| `bricks` (no subcommand) | yes | `tui::run(None, pick = false)` |
| `bricks tui` | yes | Same as above |
| `bricks resume` | yes, session picker | `pick = true` |
| `bricks resume <id>` | yes, opens `<id>` | Session resumed in its own folder |
| `bricks run …` | no | Headless JSONL. `--session <id>` continues a session. |
| `bricks sessions [--json] [--all]` | no | Read only |
| global `--workspace`/`--cd`, `--providers`, `--model`, `--reasoning` | apply to the TUI | Also used by `run` |
| global `--hyperlinks auto\|always\|never` | TUI only | Not read by `run` |
| stdin or stdout not a terminal | refused | Exit code USAGE, points to `bricks run` |
| exit | prints `Session <id> stored. Resume it with bricks resume <id>.` on stderr | |

---

## 5. Gaps and decisions for the next steps

- Events the desktop must handle for migrated functions: `search_results`, `agent_profiles`, `agent_control_result`, `skills`, `skills_trust_*`, `skill_activated`, `skill_activation_failed`, `memory_*`, `run_usage`, `usage`, `job_*`, `context_cleared`, `compaction`, `command_rejected`. The reducer may ignore some of them today.
- `@` file index: no desktop command in the contract. Needs a decision (a new command, or path search in the composer).
- `/resume <id>` must go through the desktop open path (`SlashPresent::Resume`), not `Command::Resume`.
- `/clear` needs a confirmation that explains its scope (spec §5).
- Composer history is not persisted. Resumed prompts are not in ↑ history (only cells).

## 6. Quirks kept as they are (registry semantics unchanged)

- With a pending attachment, the TUI does not parse a line starting with `/`: it is sent as prompt text. Desktop local commands must not behave this way (spec §5: a recognised local command is never sent as a prompt).
- `/agents status` without an id is a profile query for the word `status`.
- `/search re:` with an empty regex is passed to the engine as an empty regex.
- The `/changes` error and notice refer to "`changes ready`": that is the `changes_ready` notice, not a command. The desktop should list ChangeSets instead.
- `/skills page N` and `/skills <words> page N` are parsed by the registry. Other words are the query.

## 7. Build note

`cargo test -p bricks-tui` (without target names) fails: Cargo auto-discovers `crates/bricks-tui/tests/*2.rs`
(user copies, names with spaces) and stops with `invalid character ' ' in crate name`. Workaround used for the
checks: `cargo test -p bricks-tui --lib --test interaction --test render --test links --test status_bar --test totals --test workspace`.
The real fix is to remove or rename the user copies (not done here: they are user data).
