# Changelog

## [Unreleased]

### Removed — Sprint 15: disconnected APIs and historical harnesses (breaking change)

- Remove the unused agent modules `agent_tool`, `coordinator`, `auto_dream`, `session_memory` and `context_analyzer`, their example-only paths, and the inert `SystemPromptSection` cache API. Native sub-agents, compaction and structured memory remain supported.
- Remove the ignored top-level `Agents.background` argument and the always-on `tool_calls` model setting. Delete `tool_calls` from existing provider configuration files; unknown keys now fail validation. `async_tool_calls` remains supported. The desktop model contract no longer reports a constant `tool_calls` flag.
- Remove the unused desktop sidebar component family, its mobile hook and `mode-watcher`, and the historical `abstract` benchmark scripts/pages. Keep the active `tbench-agent` harness and the `@internationalized/date` peer dependency required by `bits-ui`.
- `scripts/run_tests.sh` runs one Nextest pass and propagates failures without masking pipelines. Repository metadata points to Bricks.

### Changed — the tool-result budget follows the model

- **Default budget.** Tool results kept in the active history are no longer capped at a fixed 50,000 characters: unset, the budget is the model's declared input limit read as characters (about a quarter of its window), never under 50,000. A model with a large window no longer loses each file it read to the next read and reads it again. `.tool_result_budget(n)` still sets a fixed budget.
- **System prompt.** The parallel-calls bullet now asks to group independent calls in one answer, for example reading several files already found.

### Changed — permissions: one engine, one policy per session (SDK, CLI and desktop breaking change)

- **One engine decides every tool call** (`bricks_agent::permission`), in the desktop application, the `bricks` command and the library. Each session has one `SessionPolicy`: profile ASK (reads and searches run; writes, commands, web and sub-agents are asked), EXECUTE (everything runs) or CUSTOM (an effect per action class, read, write, execute and network, and per tool; a tool's own rule wins). CUSTOM is the only profile that refuses calls. `forbidden` tools are refused in every profile. A policy never chosen behaves as ASK.
- **Storage outside `bricks.toml`.** The default for new sessions is `~/.bricks/permissions.json`, chosen once by a person. Each new session copies it into its own `permissions.json` in its files folder and keeps it. An unreadable file is an error, never a silent allow; an unreadable stored session policy stops the session from opening.
- **Revisions.** Each policy change bumps a revision. A request already waiting for a person is not answered by a change; calls decided afterwards follow the new revision (`approval.policy_revision`). Sub-agents share their parent's policy.
- **Answers cover one call.** The approval card has only Autoriser and Refuser. The `approve` decisions are `allow` and `deny`; there is no "allow for the session".
- **Desktop.** Settings › General › « Permissions des nouvelles conversations » sets the default. A first-choice bar above the composer appears while a conversation's policy is not chosen. The `/config` view edits the conversation's policy; `/tools` shows each tool's decision and its reason. The « Approbation des outils » preference is removed.
- **CLI.** `--permissions ask|execute` sets the profile of a run's session. The terminal prompt is `Allow this call? [y]es / [N]o:`. Non-interactive runs still stop with `approval_required` (exit code 3) when a call is asked.
- **Control protocol, schema 8.** Command `set_permission_policy {policy}`; event `permission_policy_changed {policy, revision}`; `approval_requested.approval.policy_revision`; `approval_resolved.by` is `user`, `non_interactive` or `cancelled`; the snapshot's `permission_policy` and `permission_revision` replace `allowed_for_session`, `approval_rules` and `approval_mode`.
- **Library.** `PermissionPolicy` is only the approver: `.permission_policy(approver)` sets it (alone, the ASK profile); `.permission_profile(SessionPolicy)`, `.permission_cell(..)` and `.permission_engine_arc(..)` are new. `PermissionRequest` gains `category` and `policy_revision`; `PermissionDecision` is `Allow`, `Deny(reason)` and `AllowOnce`. `SessionPolicy`, `PermissionProfile`, `PermissionEffect`, `CustomRules` and `PermissionVerdict` are in `bricks::prelude`.

### Removed — permission rules and "allow for the session" (SDK and config breaking change)

- `[permissions]` and `[permissions.tools]` in `bricks.toml` are no longer read. The section is reported as a diagnostic (`OBSOLETE_PERMISSIONS`) and its rules, denials included, are not applied. Delete it.
- From `bricks_tools::permissions`: `AllowReadOnly`, `RuleBased`, `PermissionRule`, `PermissionAction` and the `AllowForSession` decision. Replace `AllowReadOnly` with the approver `DenyAll` (ASK; note that it also refuses the web tools), `RuleBased` with a CUSTOM policy, and `AllowForSession` with `AllowOnce` or `Allow`.
- The « Approbation des outils » preference (Standard, Toujours demander, Sans confirmation).
- See `docs/content/docs/migration.mdx` section 16 for the steps.

### Removed — `EnterWorktree`, `ExitWorktree`, `SyntheticOutput` and `ToolSearch` (SDK breaking change)

- The four built-in tools are removed, with their modules (`worktree`, `tool_search`, `synthetic_output`) and `bricks_tools::orchestration()`. No alias: a call to one of them is an unknown tool, and the error lists the available tools. Old sessions that contain such calls still read and display them.
- Replacements: a manual worktree is `Bash` with `git worktree add -b` and `git worktree remove` (never forced). Sub-agent isolation is unchanged: `Agent` and `Agents` take `isolation: "auto" | "shared" | "worktree"`, and `AgentControl` keeps `inspect_changes`, `apply_changes` and `discard_changes`. `SyntheticOutput` had no consumer and `ToolSearch` has no replacement: the tool catalogue is already in the prompt.
- `bricks_tools::all()` is `coding()` plus `planning()`: 16 tools, where it had 20. `coding()` is unchanged (15).
- Migration: `docs/content/docs/migration.mdx`, section 18.

### Removed — built-in skills and default skill folders (SDK breaking change)

- The built-in skills `commit`, `debug`, `remember`, `simplify`, `stuck` and `verify` are removed. Bricks ships one built-in skill, `create-skills`. The memory and git features are unchanged, and those names may be used by a skill a person installs.
- Bricks no longer reads `~/.claude/skills`, `~/.agents/skills`, `<project>/.claude/skills` or `<project>/.agents/skills`, on the desktop and in the CLI, by default. Those folders are never read, copied, modified or deleted. Embedders pass sources explicitly (`SkillRoot::explicit` in `SkillsSetup::extra_roots`); `bricks_skills::standard_roots` is kept as an opt-in helper. `--trust-project-skills` concerns only explicitly configured sources. Migration: section 15 of `docs/content/docs/migration.mdx`.

### Added — skill store, installation and withdrawal

- **Skill store.** Skills a person installs live in `<bricks_home>/skills/<name>/` (`BRICKS_HOME`, else `~/.bricks`), each a portable Agent Skills folder. A hidden `.bricks-install.json` holds the installation id. Supported fields: `name`, `description`, `license`, `compatibility`, `metadata`, `allowed-tools` (advisory). Claude Code directives (`context`, `agent`, `model`, `effort`, `hooks`, `paths`, `shell`, non-default `disable-model-invocation` or `user-invocable`) make a skill incompatible: it is refused at install, never silently stripped. Store code: `crates/bricks-skills/src/store.rs`.
- **Settings › Skills.** Three actions beside the list and search: **Ajouter** (name, description, instructions, with errors under the field and the text kept), **Importer** (a folder is copied whole, all files at any depth, into a staging area validated under its final name and published in one step; the chosen folder is left intact; symbolic links and special files are refused; no script runs), and **Supprimer** (only for skills installed in the store, with an inline confirmation). `create-skills` and outside sources cannot be removed.
- **`Skill` tool actions.** `validate` checks a folder and writes nothing. `install` copies the folder into the store, validates the copy again and publishes it; it is the only action that asks the write permission. `path` gives the folder.
- **Live changes.** The settings, the composer's skill menu and open conversations (sub-agents included) see a store change at their next request, without a restart.
- **Withdrawal.** Before each request, every activation is checked against its installation, source and revision. A skill removed, reinstalled or changed is withdrawn: its instructions in the active history become a short note (tool calls and results stay paired), a sub-agent's preloaded copy leaves its system prompt, and a compaction summary written from it is set aside, with the active history rebuilt from the pre-compaction snapshots. A reinstalled skill is not reactivated silently. Limits: a session whose snapshots are missing cannot be checked this way.

### Changed — skill catalog and prompt

- The system prompt's Skill guidance mentions `validate` then `install`. The instructions of `create-skills` are loaded on activation only.

### Removed — `NotebookEdit` (SDK breaking change)

- `NotebookEdit` is removed with its module `notebook_edit` (`NotebookEditTool`) and its registration in `bricks_tools::filesystem()`. A call named `NotebookEdit` is now an unknown tool; no alias or shim is kept, and a custom tool may still use the name. Recorded history still displays it. Migration: section 14 of `docs/content/docs/migration.mdx`.

### Added — `CodeStats`

- **Code statistics tool.** `CodeStats` is a read-only tool of the FileSystem category, in `bricks_tools::filesystem()` (so in `coding()`, `all()` and `ToolSearch`). It counts the code, comment and blank lines of a file or a tree, in total, per language or per file (`summary`, `languages`, `files`), with Tokei 15 in process (`default-features = false`; no external program). Parameters: `path`, `detail`, `language`, `top_n` (1 to 100, limits the rows shown only) and `exclude` (gitignore-style, exclusions only). The walk is the shared Bricks walker plus `.tokeignore`; directory links are never followed; each file is opened without following a final link and checked against the walk before any byte is read. The JSON carries `totals`, `results`, `truncated`, `omitted`, `partial` and `warnings`; `partial` is set only when something expected was not counted. Identical analyses running at once in one session share one walk, and nothing is kept after it ends.
- **Prompt.** The Bricks foundation names `CodeStats` in one bullet, kept only when the tool is announced: it says to check `partial` before presenting totals as complete.

### Removed — fifteen native tools (SDK breaking change)

- `EnterPlanMode`, `ExitPlanMode`, `CronCreate`, `CronList`, `CronDelete`, `TaskCreate`, `TaskGet`, `TaskUpdate`, `TaskList`, `TaskStop`, `TaskOutput`, `RemoteTrigger`, `Config`, `AskUserQuestion` and `Sleep` are removed with their modules (`plan_mode`, `cron`, `tasks`, `remote_trigger`, `config_tool`, `ask_user`, `sleep`) and registries. `bricks_tools::all()` has 22 tools (was 38; `SendMessage` is now `bricks_agent::agents::SendMessageTool`, registered with the agent tools, and `AgentControl message`, `bricks_tools::send_message`, `MessageRouter` and `MessageRouterHandle` are removed); `scheduling()` is removed and the system prompt has no Task tracking section. A call to a removed tool is an unknown tool. Replacements: `docs/content/docs/migration.mdx`, section 12.

### Added — plan, request context and file icons (Sprint 14.8)

- **Durable plan (TodoWrite).** The conversation's plan is written atomically to `plan.json` in its files folder, with a revision per update, shown as a collapsible block in the desktop, and sent as the new `plan_updated` event (JSONL schema 7). Sub-agents never receive the parent's plan; a session without a plan starts with none. A plan whose run ended with its answer, or whose steps are all done, is cleared when the next message is accepted; after an interruption, a failure or a reopened conversation, unfinished steps stay. A clearing that cannot be saved never refuses the message.
- **Per-request system prompt.** The Bricks prompt is assembled for each request from the tools that request announces. Its dynamic part, after the cache boundary, carries the date, environment, model (API name, reasoning profile), git state and plan. Data in these blocks and skill activations is XML-escaped. Providers may report `request_identity`.
- **File icons (desktop).** A local copy of the Logoscrap catalogue (132 SVGs, licences and provenance under `apps/bricks-desktop/public/file-icons`) is resolved offline by file name, extension and folder, in the interface and in Markdown code block headers.

### Changed — crates renamed

- The SDK is the **Bricks ADK**. The facade is `bricks` (was `cersei`) and the crates use the `bricks-` prefix (`bricks-agent`, `bricks-tools`, …; `bricks_*` imports). Unchanged: `CerseiError`, `CERSEI_*` variables, `.cersei`, `cersei-envd`, `tbench-agent`, Docker names. The table of names, imports and every behaviour change of this release are in `docs/content/docs/migration.mdx`.

### Changed — file edits

- **No read is required before a change.** The runtime no longer refuses `Edit`, `Write`, `NotebookEdit` or `ApplyPatch` on a file the model has not read in the session (the read-before-edit guard is removed). A change is still decided on what it would do to the file, and the tool descriptions and error messages recommend reading the code when a change needs it; none of them requires it.
- **`Edit` takes several replacements.** `Edit` accepts `{file_path, edits: [{old_string, new_string?, replace_all?}, …]}` next to the one-replacement form `{file_path, old_string, new_string?, replace_all?}`; the two forms cannot be mixed. The replacements are applied in order, each against the result of the previous one, and the file is written once, only if all of them apply. `file_path` is the only required parameter. An omitted `new_string` deletes the match, as before.
- **Per-file write lock.** `Edit`, `Write`, `NotebookEdit` and `ApplyPatch` hold a lock on each file they write from the read to the write (`cersei_tools::file_lock`). The lock is keyed by canonical path and shared by every session and agent of the process, so two edits of different parts of one file no longer overwrite each other, and a symbolic link and its target share one lock. Locks are never held while an approval is pending.
- **The approval is checked where the write happens.** The approval records the SHA-256 of the text the diff was computed from. Under the file's lock, before writing, the tool compares the file with that state: a file changed since its approval is not written, and the call is decided again on the file as it now is. A hook that changes a tool's input makes the call decided again when its change differs from the approved one, so the change written is the change approved.
- `Write` writes through `write_atomic` (a temporary file, then a rename).
- The system prompt and the tool descriptions no longer state a read as a precondition of an edit.

### Removed — `MultiEdit` (SDK breaking change)

- The `MultiEdit` tool is removed: `cersei_tools::multi_edit` (`MultiEditTool`), its registration in `cersei_tools::filesystem()`, its schema and tests. A call named `MultiEdit` is now an unknown tool. Recorded history that contains `MultiEdit` calls still reads and displays.
- Migration for SDK users: replace `MultiEditTool` with `cersei_tools::file_edit::FileEditTool` and send the same arguments under the name `Edit`: `{"file_path": "…", "edits": [{"old_string": "…", "new_string": "…", "replace_all": false}]}`.

### Removed — `delegate` (SDK breaking change)

- The `delegate` tool is removed: `cersei_agent::delegate_tool::DelegateTool`, the batch module `cersei_agent::delegate` (`run_batch`, `DelegateConfig`, `DelegateTask`, `DelegateResult`, `DELEGATE_BLOCKED_TOOLS`, `DEFAULT_MAX_CONCURRENT`, `ProviderFactory`) and their registration. A call named `delegate` is now an unknown tool. Recorded history that contains a `delegate` call and its result stays readable.
- `ToolsetFactory` moves to `cersei_agent::agent_tool` (same type). `MAX_DEPTH` is private there; the sub-agent depth rule is unchanged.
- `subagent::legacy_admission` takes no count any more: an `AgentTool` sub-agent holds one slot.
- Migration for SDK users: start one sub-task with the native `Agent` tool (`task`, `background`, optional profile) or several with `Agents` (`agents`, `fail_fast`). Their parameters are not those of `delegate` (`goal`, `tasks`, `context`, `workspace`) and no equivalence is promised: rewrite each task as a self-contained `task`.

### Changed — waits, limits and retries

- **Process stops are notified, not polled.** Stopping a tree waits for the kernel's report of each exit (`kqueue` `NOTE_EXIT` on macOS, `pidfd` in an `epoll` set on Linux) through a watcher thread, with the same grace, forced stop and bounds. Where no notification is available, a bounded `kill(pid, 0)` poll remains. Background tasks announce readiness to their waiter instead of being polled every 20 ms.
- **Workspace snapshots** take one copy pass and two state reads. Each copied file is hashed as it is written and checked before and after the copy (size, modification and change time, inode, link target). The retry sleep is removed; three attempts and the refusal stay.
- **LSP start failures and shutdown** read the server's stderr to its end (at most 500 ms more), instead of a fixed 50 ms wait.
- **Defaults.** `[agents] background_drain_ms` is `0` (was 600000): background sub-agents still running when the main run ends are cancelled at once (`bricks run` exits with code 6). `[agents] admission_timeout_ms` is `30000` (was 600000).
- **`[agent] retry_after_max_wait_ms`** (default `60000`, new): a provider's `Retry-After` longer than this is not waited for. No retry is sent; the error says so.

### Changed — sub-agent limits

- **Defaults.** `[agents] summary_chars` is `30000`: the answer returned to the parent, the ellipsis included; the full transcript stays on disk. `[agents] max_depth` is `1` (was `2`): the session's agent starts sub-agents, which cannot start their own unless `max_depth` is raised in `bricks.toml`. The output of a tool call inside a sub-agent event is cut to 96 KiB (was 64 KiB), with `output_truncated`. No other limit changes.

### Added — SDK

- `cersei_tools::file_lock` (`lock_paths`, `lock_key`, `FileLocks`).
- `cersei_tools::preview::{ApprovedState, ensure_still_approved, is_stale_approval, STALE_APPROVAL_KEY}`.
- `cersei_tools::file_edit::{EditCall, EditSpec, parse_call, prepare}`.

### Compatibility

- `Processed::partial_view` is still produced by the output compressor but no longer decides what the runner treats as read.
- `tool_primitives::fs::edit_file` is kept as it was; the `Edit` tool no longer uses it.

### Added — revising a sent message (control protocol)

- **Command `revise {user_index, original, prompt}`.** Replaces a past user message and runs again from it. `user_index` is the message's index among the user-role messages of the active history; `original` is its text as shown. The active context is cut before that message; the context as it was is kept as a snapshot and stays in the raw history. Refused with `command_rejected` and nothing changed while a run is active, when the message is not in the active history with that text (the history changed, or it was compacted or cleared away), or for the reasons of `submit`.
- **Event `history_revised {user_index, messages_removed}`**, sent before the new run's `run_started`. A skill activated only in the removed part is dropped from the session's ledger.
- Both are additions a consumer can ignore: the schema stays at 8. `docs/cli.md` has the command and the event.

### Added — desktop: revisions, moved projects, search

- **Editing a sent message.** « Modifier » and « Copier » on a sent message; « Enregistrer et relancer » sends a `revise`, « Annuler » leaves the message. The run records, run labels and attachment records of the replaced messages are set aside in `<session>.revised.jsonl` and `attachments.revised.jsonl`. New IPC command `session_run_user_index`.
- **Projects whose folder moved.** A project whose folder is missing stays in the list, with its conversations and a red icon. Opening one of them, « + », a search result or the first message asks for the folder (« Dossier de travail introuvable », « Choisir un dossier… »). The project keeps its id and its conversations follow it. New IPC command `project_relocate`; its journal is in `<home>/desktop/relocations/`, finished or undone at the next start.
- **Search** is a magnifier beside « Nouvelle conversation », a dialog over the titles and messages of every project. The « Tous les projets » switch is removed.
- **Working time and copies.** « Travaille depuis … » while a run works. « Copier » of a response is placed after the run's summary.

### Changed — desktop: sidebar and window

- **macOS title bar.** The window uses an overlay title bar; the top bar is 52 px high and leaves room for the system buttons, except in full screen.
- **Scrolling.** The page does not bounce; the scrollable panes contain their overscroll.
- **Sidebar.** A long conversation title fades out instead of ending with an ellipsis; more space between projects.

### Added — `ViewImage` and images returned by tools

- **`ViewImage` tool.** Shows the model an image file: PNG, JPEG, GIF or WebP, with the type read from the bytes, never from the name. The model receives the image itself; the result text names the path, the type, the dimensions (not for WebP, whose header is not read) and the size. Read-only, in `bricks_tools::filesystem()`, so in `coding()`, `all()` (now 23 tools) and `ToolSearch`. Refused: a directory, a file that is not one of these types, more than 3,750,000 bytes, and more than 8,000 pixels on a side. A relative `file_path` resolves against the working directory.
- **Images in a tool result.** `ToolResult.images: Vec<ToolImage>` (`bricks_tools::ToolImage`: media type, base64 data, path, byte size), set with `ToolResult::with_images`. The agent carries them as the model's `ImageTransport` (`Provider::image_transport()`) allows: `ToolResult`, inside the tool result (`anthropic_messages` with image input); `UserTurn`, with the result kept as text and the round's images after all its results, in the same user turn, each after the label `[image from tool call <id>] <path>, <type>, <n> bytes` (`chat_completions` and `responses` with image input); or `None`, with the note `[the image was not shown: the current model does not accept images]` in the result text (no image input). Image input comes from `input_modalities` in `providers.toml`. A provider that does not override `image_transport()` gets `None`. `ToolResult` has a new public field: code that builds it with a struct literal adds `images: Vec::new()`; the constructors are unchanged.
- **Desktop.** A successful `ViewImage` call shows a thumbnail under its line: the image the model received, read from the open session's history by the new IPC command `session_tool_image`. Reopened conversations no longer show the text and images of tool-result turns as user messages.
- **MCP** image results are unchanged: they stay text placeholders, `[image <type>, <n> bytes — not shown]`.

### Removed — `BashTaskStatus`, `BashTaskOutput` and `BashTaskStop` (breaking change for SDK and tool users)

- The three tools are removed, with no alias: a call to one of them is an unknown tool. Recorded conversations still display their calls. Migration: section 17 of `docs/content/docs/migration.mdx`.
- `bricks_tools::shell()` no longer contains them, nor `Job`: it is `Bash` and `PowerShell`. `bricks_tools::all()` has 20 tools (was 23) and `coding()` has 15 (was 18).

### Changed — `Job` follows every background job

- `Job` is the only tool for a command started with `background: true`, and for a foreground command handed over after `yield_time_ms`. Both results give a `job_id` (`job_` and 12 hexadecimal characters). The `task_id` (`bg-N`) is no longer returned.
- Actions: `list` (default), `status`, `output`, `wait` and `stop`. `output` takes `stream`, `offset` (0-based lines), `limit` (1 to 2000, 200 by default) and `tail` (the last `limit` lines, not with `offset`). `wait` takes `timeout_ms` (1 to 600000, 60000 by default). Unknown fields and out-of-range values are refused, never clamped.
- An id that is unknown, or belongs to another agent, gets `No accessible job with this id`. A job is visible only to the agent that started it.
- `output` pages are bounded in bytes. A line longer than the page is cut and marked, and the offset moves past it. A line still in progress is shown apart and does not move the offset. `status` gives the raw log paths: the live logs while the job runs, the durable copies once kept.
- `stop` is TERM to the process group and tree, then KILL after the grace period; it is idempotent, and a job that already ended is a success. `wait` with a timeout stops nothing.
- `Bash` names `Job` in its description and results. `yield_time_ms` only gives control back; `timeout` still ends the command.

### Changed — job registry assembly (SDK)

- `bricks_tools::shell_with_jobs(registry)` builds `Bash`, `Job` and `PowerShell` on one `JobRegistry`. `coding()` and `all()` use it on a fresh registry. `on_job_registry(tools, &registry)`, `Tool::job_registry()` and `Tool::on_job_registry()` move a set of tools onto one registry.
- `Bash` without a registry (`shell()`) refuses, before anything starts, `background: true` and any command that may be handed over (a `yield_time_ms` below its `timeout`). Give `yield_time_ms` at least the `timeout` to have the call wait for the end.
- `AgentBuilder::build` puts the job tools on the `JobsHandle` of the agent's extensions (sessions and sub-agents have one), or makes the tools' registry the agent's `JobsHandle`. Tools on two different registries are refused with `CerseiError::Config` before any model call.
- `Task::read(stream, OutputPosition::Offset(n) | OutputPosition::Tail, limit)` returns a `Page`; its `evicted` counts the lines no longer in memory.

## [0.6.1] — 2026-10-14

### Changed — system prompt (Sprint 13.2)

- **New Bricks foundation.** The built-in prompt opens with one fixed text (in French): the Bricks introduction, then working with judgement, using the tools, acting within what is authorised, verifying in proportion, communicating clearly. It replaces, without keeping copies, the former blocks: the "built with/on the Cersei SDK" attributions (and the "running with custom instructions" variant), `CORE_CAPABILITIES`, `TOOL_USE_GUIDELINES`, `ACTIONS_SECTION`, `SAFETY_GUIDELINES`, `SECURITY_SECTION`, `SCOPE_AND_STOPPING`, `OUTPUT_EFFICIENCY`, `SUMMARIZE_TOOL_RESULTS`, `CODE_SCOUT_GUIDANCE` and `SESSION_AGENT_GUIDANCE`. Gone with them: the fixed 2-minute Bash timeout, "dedicated tools instead of Bash", "never delete files without confirmation", "one sub-agent at a time / a sub-agent cannot start another" (native children do have the delegation tools), and the requests to write tool results down in the answer.
- **Two conditional bullets, one text.** The CodeScout bullet is kept only when `CodeScout` is announced to the model; the delegation bullet only when a tool that starts a sub-agent is (`Agent`, `Agents`, `delegate`: `system_prompt::SPAWNING_TOOLS`) — `AgentProfiles`, `AgentControl` or `TaskCreate` alone do not count. With both, the foundation is exactly the reference text (`crates/cersei-agent/tests/fixtures/bricks_foundation.txt`).
- **Same identity in every standard mode**: interactive, non-interactive, non-interactive with appended instructions. `SystemPromptPrefix` stays; `attribution_text()` returns the Bricks introduction.
- **Sub-agents** get the same foundation, decided by their own tools, then a `## Your role` section (moved from `CHILD_FRAMING` into the builder's cacheable part, `prefix: SubAgent`), then their profile's instructions and preloaded skills. One role section, not two.
- Specialised sections reduced to what they describe: `TodoWrite`/`TaskCreate` (no obligation to keep a list), coordinator mode (orchestrator role, no parallel mandate, no tool named), context management (old results may be removed and name their saved original; no request to copy them). Skills, memory, output style, language, custom and appended instructions, extra sections, working directory, Git, memory content, MCP instructions and the dynamic boundary are unchanged.
- `replace_system_prompt: true` keeps its contract: the integrator's text alone, then the boundary, without the foundation.
- The CLI announces to the prompt builder the sub-agent tools the controller really adds (`Agent`, `Agents`, `AgentControl`, `AgentProfiles` when `[agents] enabled`).
- No change to permissions, tool execution, stop, resume or delegation rules.
- New public items: `system_prompt::BRICKS_INTRODUCTION`, `bricks_foundation`, `SPAWNING_TOOLS`.

### Added — interface (Sprint 13.1)

- **Run timer next to `working`**: `⠦ working [12 s] (Ctrl+C to cancel)`, then `[1 min 05 s]`, `[1 h 02 min 03 s]`, `[1 d 02 h 03 min 04 s]` (SI symbols `s`, `min`, `h`, `d`, a space before the unit, two-digit sub-fields, rounded down: never `60 s` or `60 min`). It measures the run's wall time from `run_started` (tools, approvals, retries, compactions included), with the monotonic start the interface already kept; it is redrawn at the existing tick (no timer task), starts again with each run, disappears at the end or cancellation, and a session opened never inherits a start. Narrow bars keep activity, time and cancellation first (`working [1 min 05 s] (Ctrl+C)`, `[1 min 05 s] (Ctrl+C)`, `[1 min 05 s]`). A dedicated formatter (`statusbar::elapsed`); tool, command and event durations stay in milliseconds (`display_ms` unchanged).
- **Ctrl+C cancels a `/compact` in progress** (outside a run it used to arm the quit).

### Changed — responsiveness and cancellation (Sprint 13.1)

- **Tool calls are published as they finish.** A round's calls still run in parallel (permissions, read-before-write guard, writer admission and the persistent shell's serialization unchanged), but they are collected as they end (`FuturesUnordered`) instead of after the slowest one: `edit_applied` and `tool_finished` of a fast call no longer wait for a slow sibling. The history sent to the model, the raw history, the call records, the failure streaks and the progress check keep the order of the calls; the next request is sent once the whole round is resolved. A `Skill` activation is still announced only once committed to the context. Output reduction runs on the blocking pool. One visible difference: the failure-streak note of a failed call is in the result sent to the model (under `--- remarques ---`), no longer appended to the `tool_finished` output.
- **A cancellation keeps finished calls.** Their results (and the edits they reported, counted once) stay in the history; calls still running are dropped through their guards and get the usual interruption result; sub-agents are settled as before. Nothing is invented for an effect that did not happen.
- **Compaction and remote counting can be cancelled and are bounded in time.** A compaction attempt (automatic, manual, overflow recovery) watches the cancellation while waiting for the headers and while reading the stream, and has an absolute limit of 300 s (`compact::COMPACTION_ATTEMPT_TIMEOUT`); the counting endpoint has 30 s, then the local estimate is used. New outcome `CompactionOutcome::Cancelled`: history kept, not counted as an unsuccessful attempt, nothing sent after it (no fallback, no resend). `Agent::compact()` has an operation token of its own, new at each call, cancelled by `Agent::cancel()` and the `cancel` command (also outside a run). No global timeout on the shared HTTP client. `compact_history_with` takes the token and the deadline; `compact_history` keeps its signature.

### Fixed — search and storage (Sprint 13.1)

- **CodeSearch** builds its index from one read of the sources (it used to collect the chunks twice), with BM25, metadata and vectors from the same snapshot of chunks, on the blocking pool (at most two builds at once). The index is keyed by canonical root and embedding configuration (it used to be one global index keyed by the path only, shared by BM25-only and hybrid tools), and reused only while an inventory of the sources (path, size, modification time, with the workspace's ignore rules) is unchanged and no writing tool ran since its build began (`code_search::invalidate`, called with the CodeScout notification, only for calls that actually ran). Concurrent searches share one build; a build invalidated meanwhile is not published; a build whose callers went away stops at its next file and publishes nothing. Vectors of unchanged chunks are reused; an embedding answer of the wrong size falls back to BM25. A relative `path` is now relative to the working directory. Limit, documented: an external edit keeping both size and modification time is not seen by the inventory.
- **Glob** walks with the shared ignore-aware walker (`.gitignore`/`.ignore`, re-inclusions, no `.git` needed; ignored directories pruned before descent; hidden entries only when the pattern names one), takes `path` literally, and stops at the first match past `limit` (`tool_primitives::search::glob_walk`, `GlobOptions`, `GlobOutcome`). The output says `Showing the first N matches; more matches exist. Narrow the pattern.` instead of a total it can no longer know; a walk is bounded at 200 000 entries and stops when the call is cancelled. `limit: 0` lists nothing without walking. `search::glob` now follows the same rules. The tautological test is replaced by tests on fixtures.
- **Live Markdown is rendered once per change.** The live area reuses each answer's parsed and wrapped lines between frames (ticks, other cells' events); while an answer streams, only its last open block is parsed again (stable block boundaries; documents with a link reference definition or an HTML block are rendered whole), and a resize re-wraps without parsing. A closed answer is rendered whole by the same renderer. Queued events are applied before the next frame. Cache entries leave with the scrollback commit and at a session change.
- **Session histories are saved as one pair.** `save_session` copies the active and raw histories together, releases the locks, and stores them through `Memory::store_histories`; restoration reads `Memory::load_histories`. With `JsonlMemory` the pair is one checkpoint, `<id>.jsonl` (header, active, raw), written to a temporary file, synced, then renamed (and the directory synced): an interrupted save leaves the previous complete pair. Every other `JsonlMemory` write is atomic too (no more truncated file). Publications of a session are serialized per session; an older save of the same owner never replaces a newer one, and an unchanged pair is not rewritten. Older sessions stay readable; the next save of a session migrates that session only. The sync has a cost: on a fixed corpus (400 active + 1,200 raw messages, 1.8 MB), a save took about 15 ms instead of 1.3–2.2 ms for the two unsynced rewrites, and an unchanged pair about 3.4 ms (serialization and digest, no write) — macOS/APFS, release build, 3 × 20 runs. This cost is at the end of a request, not before its first token.

### SDK

- `Memory` gains `store_histories`, `load_histories` and `histories_are_atomic`, with default methods (two separate stores, no transaction) so existing implementations compile; `JsonlMemory` and `InMemory` override them. New types `SessionHistories`, `SaveRevision`, `HistoriesSaved`.
- `CompactionOutcome::Cancelled`; `compact::compact_history_with`, `compact::COMPACTION_ATTEMPT_TIMEOUT`.
- `bricks_tui::render_cache`, `markdown::render_rich_part`, `markdown::last_stable_boundary`, `markdown::needs_whole_render`, `statusbar::elapsed`, `statusbar::status_line_at`; `App::render_cache`.
- JSONL schema unchanged (5): the `compaction` event's `outcome` text can read `cancelled: …`.

## [0.6.0] — 2026-10-11

### Changed — Rust edition 2024

- The workspace (and `examples/benchmark`) moves from edition 2021 to **edition 2024**; a recent stable Rust is required (1.88 or later for let chains; tested with 1.98.1).
- Migration by `cargo fix --edition`, then reviewed:
  - `if let … else` with temporaries rewritten as `match` where the drop scope changes;
  - `+ use<…>` on a few `impl Trait` returns;
  - nested `if`s merged into let chains (Clippy).
- Code formatted with the 2024 style (`cargo fmt`).
- The remaining drop-order notes were checked; none changes a lock's purpose. The workspace writer gate keeps its guard for the whole tool call; LSP and controller guards are released earlier than before, after their use.
- The MCP configuration tests no longer modify the process environment (`std::env::set_var` is `unsafe` in 2024, and tests run on several threads). `expand_env_vars_with` and `expand_server_config_with` take the variables from a lookup function; `expand_env_vars` and `expand_server_config` are unchanged.

### Added — interface and reliability (Sprint 12.5)

- **`/effort`**: the reasoning profiles of the current model, from the providers file (ids, labels, parameters; no built-in list). `/effort <id>` selects one; `/effort default` goes back to the **model's declared default** (not "off", not an earlier choice). A profile whose id is `default` stays selectable in the picker. An unknown id is refused with the choices and changes nothing; a model without profiles says `No reasoning profiles configured for this model.`
- **Contract**: command `set_reasoning {profile?}` (absent/null: the model's default) and event `reasoning_changed` (effective profile before/after, ids and labels). `session_opened` and `model_changed` gain `effective_reasoning`; `Snapshot::effective_reasoning`. `set_model` keeps its meaning. Additions: JSONL schema stays 5.
- **`/cost`** and **`/context`** rewritten as readable summaries, plus `/cost details` and `/context details`. Any other argument is refused.
  - Cost has distinct states: an estimate, `$0.00` for a known zero, `Not priced`, and `Not priced (partial)` with the known subtotal labelled as such and the unpriced categories. A small amount never rounds to zero (`<$0.0001`).
  - Cache read/write are kept apart from input. Reasoning is shown as part of output and never added to it.
  - Context separates occupation from the session total. The `≈` mark depends on provenance; occupation is measured against the prompt budget and may exceed 100%. A window that is not declared reads `Not configured`.
  - Two distinct availabilities: safe (`budget − margin − upper bound`) and raw (`budget − used`).
- **Adaptive status bar**:
  - Model name and effective profile label, context, cost, activity and project, chosen by the width actually available, measured in terminal cells and cut between graphemes; control characters are neutralised.
  - The activity (`working`, `Ctrl+C`) stays during a run. The `⌂` decoration is gone.
- **Optional pre-commit hook** (`scripts/hooks/pre-commit`, enabled only by `scripts/hooks/install.sh enable`). It checks the staged content (whitespace and conflict markers, rustfmt, secrets with the staged allowlist), modifies nothing, and never replaces an existing hook or `core.hooksPath`.

### Fixed (Sprint 12.5)

- **Concurrent model/profile switch**: provider, model label and profile now change together and are read as one snapshot per request. A request never combines an old provider with a new profile; a retry keeps its request's selection.
- **Accounting of a response sent before a switch**: its usage counts once in the totals, but no longer becomes the context measurement of the new selection (the measurement is kept only if the context version is unchanged since sending).
- The model picker's "(default)" for the current model kept the previous profile (`set_model` without a profile). It now clears it, through `set_reasoning`.
- Per-response usage rows (`/cost details`) no longer carry over from another session after a resume.
- User-facing `cost unknown` labels replaced (`Not priced`, with the partial state); JSONL fields unchanged.
- Shell tests: the HTTP server fixture no longer does a reverse DNS lookup (`HTTPServer.server_bind` → `getfqdn`), which could hang on some machines. A test proves the fixture starts, answers and stops with `socket.getfqdn` failing. Production supervision unchanged.
- `scripts/check_secrets.py --staged` reads the allowlist from the index.

### Dependencies (Sprint 12.5)

- **Version 0.6.0** across the workspace (Sprints 12 and 12.5).
- Updated: `uuid` 1.28.0, `serde_json` 1.0.152, `tokio-util` 0.7.20, `smallvec` 1.16.3, `indoc` 2.0.8, `zstd-sys` 2.1.1, `cc` 1.7.0, `cxx` family 1.0.205.
- Deferred: `nix` 0.31 (via `mac_address`), `tree-sitter-language`, `syn`, `pest` (new native crates).
- `cargo audit` before and after: 0 vulnerabilities, the same four advisories. Details in `docs/maintenance.md`.

### Added — Agent Skills (Sprint 12)

- **Standard skills** (`SKILL.md`, [agentskills.io](https://agentskills.io/specification)): `name` and `description` required; `license`, `compatibility`, `metadata`, `allowed-tools` optional. Name rules follow the reference validator: NFKC normalisation, Unicode letters, lowercase, no leading, trailing or doubled `-`, equal to the directory name. LF/CRLF and multi-line values are accepted. Malformed YAML, duplicate keys, tags and oversized or deeply nested documents are refused; nothing is repaired. Bricks' own limits: 100 KiB per `SKILL.md`, 32 KiB of frontmatter.
- **Sources**, by priority: `<project>/.agents/skills`, `<project>/.claude/skills`, `~/.agents/skills`, `~/.claude/skills`, then built-in.
  - Discovery is deterministic and bounded; symbolic links are not followed.
  - Collisions and diagnostics are visible.
  - An invalid winner never falls back silently to a homonym.
- **Trust of project skills** per session, decided by a person (`/skills trust`, `/skills deny`, the `trust_project_skills` command) or by an explicit caller (`--trust-project-skills`, `SkillsSetup::trust_project`, `Controller::trust_project_skills`).
  - Before that, nothing of them reaches the model.
  - Asked once per interactive session; never awaited in headless.
  - Not kept across sessions or projects.
- **Activation.**
  - The system prompt has a catalog of names and descriptions (escaped, bounded at 100 entries / 16 KiB, saying what it omits).
  - The `Skill` tool (read-only level) has `activate`, `list` and `resource` actions; `args` is a separate request.
  - The engine records an activation only on the verified path (a one-time id of the agent's own tool), once it fits in the context (compacting once if needed), and never shortens it. The same revision is not repeated.
  - Active instructions survive compaction (re-added once) and the tool-result budget.
  - They are stored with the session (`skills.json`: identity, source, revision) and re-checked against their source on restoration (suspended, and said so, if changed or no longer allowed).
- **A person's choice:** `/skill <name> <request>` and `bricks run --skill <name>` (repeatable) activate before the request, which stays separate. One that cannot be used refuses the command and sends nothing (`bricks run`: exit 2). `/skills [words | reload]` lists sources, states, collisions and diagnostics.
- **Sub-agents.**
  - A profile's `skills` are resolved with the session's service and trust before the child is built and before any provider call.
  - They are preloaded whole; a missing, refused, invalid or incompatible skill, or a total over `skills_max_bytes`, refuses the launch.
  - Each child has its own activations and never sees the parent's. A worktree child reads its project's skills in place.
- **Events** (JSONL schema 5, additions): `skill_activated`, `skill_activation_failed`, `skills`, `skills_trust_requested`, `skills_trust_resolved`. **Commands:** `list_skills`, `reload_skills`, `trust_project_skills`; `prompt.skills` in `submit`.
- Built-in skills rewritten as standard documents: `commit`, `debug`, `remember`, `simplify`, `stuck`, `verify`. Example skills in `examples/skills/`. Documentation: `docs/skills.md`.

### Changed

- **Crate renamed: `cersei-skills` → `bricks-skills`** (re-exported as `cersei::skills`). It is now the only skills service; `cersei-tools`, `cersei-agent` and the facade depend on it.
- `Prompt` gains `skills` (default empty). `UserInput` gains `skills`. `AgentBuilder::skills` / `preloaded_skills`; `EngineConfig::skills` (`SkillsSetup`). `Controller::skills`, `Controller::trust_project_skills`.
- Agents built by the CLI have the `Skill` tool and read `~/.agents/skills` and `~/.claude/skills`.
- Unsupported Claude Code directives (`context`, `agent`, `model`, `effort`, `hooks`, `paths`, `shell`, `disable-model-invocation: true`, `user-invocable: false`) make a document *incompatible*: inspectable, never activated or preloaded.
- `bricks run`: a refused submission now reports its `command_rejected` even when other events were queued before it.

### Removed

- The second skills system in `cersei-tools` (`cersei_tools::skills`: its parser, discovery, `$ARGUMENTS` templates, bundled aliases), `SkillTool::new().with_project_root(...)` / `with_extra_path(s)`, and `cersei_agent::agents::spawn::load_skills`.
- The old registry API (`SkillRegistry`, `SkillFrontmatter` with required `version`/`license`, `.cersei/skills` discovery).
- `.claude/commands/*.md` and `.cersei/skills` are no longer read. The `loop` built-in is gone (Bricks has no scheduler). See the migration table in `docs/skills.md`.
- Profile skill states `truncated`, `missing`, `invalid`, `skipped` are no longer produced: such a launch is refused.

## [0.5.0] — 2026-10-10

### Security — dependencies (Sprint 11.1)

- **Hickory 0.26.3**: `hickory-proto` 0.25.2 (RUSTSEC-2026-0118, RUSTSEC-2026-0119) is gone from the graph. reqwest stays at 0.12 without its `hickory-dns` feature; one Hickory 0.26 resolver is injected into every HTTP client through `cersei_types::http` (providers, web, MCP, embeddings, the HTTP tool). Still a Rust resolver (static musl builds do not depend on `getaddrinfo`); SSRF filtering checks the same resolution. On macOS, a scoped link-local nameserver (`fe80::…%en0`) that Hickory 0.26 refuses is skipped with a warning; no public DNS is ever added.
- **tantivy 0.26.2, notify 8, portable-pty 0.9**: removes lru 0.12.5 (RUSTSEC-2026-0002), `instant` and `serial`. `cargo audit`: 9 advisories before (2 vulnerabilities, 2 unsound, 5 unmaintained), 4 after (0 vulnerabilities; lru 0.16.4 RUSTSEC-2026-0253 not reachable from tantivy's `usize` keys; bincode, fxhash, paste unmaintained). Details: `docs/maintenance.md`.
- `reqwest-eventsource`, unused, is removed.

### Fixed (Sprint 11.1)

- **SSE decoder**: a block with `event:` but no `data` is no longer dispatched, and its name no longer leaks into the next event; `data:` with an empty value is still an event. One event (data, name, current line) is bounded to 16 MiB; comment lines are skipped without being stored. Beyond the bound the response ends with an explicit protocol error that reaches the runner — not retried, not replayed. The end-of-stream flush of a last event without a blank line is kept and documented.
- **Bash analysis** (`tool_primitives::bash_safety`, an SDK primitive not wired into permissions): writes (`>`, `tee`, `sed -i`), interpreters and `sh -c` (whose literal script is analysed), `find -exec`/`-delete`, `xargs`, `awk system()`, `env`/`command`/`sudo`/`timeout` wrappers, git mutations (`branch -D`, `stash pop/drop/clear`, …), recursive deletion of `/` or the home folder in all its spellings, parse errors and dynamic names or targets are no longer reported as safe. `rm -rf` of an ordinary folder is `High`, not `Forbidden`. The Bash tool's approval (`Execute`) is unchanged.
- **CodeSearch** indexed nothing when the workspace folder's name starts with a dot.

### Removed (Sprint 11.1)

- **`cersei_tools::bash_classifier`** (`classify_bash_command`, its `BashRiskLevel` with `Low`/`Medium`/`Critical`, `to_permission_level`): a second, substring-based classifier that nothing in production used. Use `tool_primitives::bash_safety::analyze_command`.
- The Linux benchmark binaries, two `.pyc` files and `failure_patterns.txt` are no longer tracked (local copies kept; how to rebuild or recover them: `bench/term-bench/BINARIES.md`).
- `docs/bun.lock`: the documentation site uses npm only (`package-lock.json`, `packageManager` set).

### Changed — repository (Sprint 11.1)

- **Version 0.5.0** across the workspace.
- `Cargo.lock` is versioned; validations use `--locked`.
- `sync_cargo.sh` publishes `bricks-semantic` and `cersei-web` too, and checks its list against `cargo metadata` before anything (`scripts/check_publish_order.py`, tested by `scripts/test_sync_cargo.sh`). Nothing was published.
- `scripts/check_secrets.py` finds key-shaped values in staged or tracked files without printing them; `docs/secrets-history.md` lists the history's findings by fingerprint, with rotation and purge steps (not carried out).
- Documentation: the crate map names `bricks-cli` and `bricks-tui`; pages of the old `abstract` CLI and provider system are marked historical, and install instructions point to `cargo install --path crates/bricks-cli`. The README shows version 0.5.0; its benchmark figures stay those measured on 0.4.6.

## [0.4.8] — 2026-10-08

### Removed — turn limits (Sprint 11.02)

- **No agent has a turn limit any more**: the SDK (`Agent::builder`, formerly 10), the session agent of the CLI and controller (formerly 50), native sub-agents (`Agent` / `Agents`, foreground, background, descendants, shared or worktree; formerly 30 by default, 100 at most), and the historical paths (`AgentTool`, `DelegateTool`, `delegate::run_batch`, `cersei-agentrl`'s general, proposal and replay agents, `tbench-agent`). A run goes on until its answer and stops otherwise only by the engine's other rules: cancellation (and its propagation to children), definitive errors, no progress, refusals, empty or cut answers. Depth, concurrency, per-run totals, admission, jobs, tool timeouts, context checks and the 0.4.7 network retries are unchanged. No hidden budget replaces the limit; the turn counter saturates and never stops a run.
- **API removed**: `AgentBuilder::max_turns`, `AgentTool::with_max_turns` / `with_max_turns_cap` and `DEFAULT_SUBAGENT_TURNS(_CAP)`, `DelegateTool::with_max_turns`, `DelegateConfig::max_turns`, `CerseiRunner::with_max_turns` / `with_general_max_turns`, `AgentSettings::max_turns`, `DelegationSettings::{default_max_turns, max_turns_cap}`, `AgentProfile::max_turns` and `MAX_PROFILE_TURNS`, `Resolved::max_turns`, `SpawnInfo::max_turns`; the `max_turns` argument of the `Agent` / `Agents` tool schemas and of the legacy `Agent` tool; `tbench-agent --max-turns`.
- **Migration**: `[agent] max_turns`, `[agents] default_max_turns` / `max_turns_cap` and a profile's `max_turns` are accepted, ignored and reported as diagnostics ("remove it"); every other key is checked as before; files are never rewritten. A tool call or `tbench-agent` run that still passes `max_turns` is refused with a migration message, before anything starts. `Termination::MaxTurns` is kept only to read results stored before; no run produces it. Old instance manifests with `max_turns` still load.
- **JSONL schema 5**: `agent_spawned` no longer has `max_turns`; `edit_applied` gains `agent_id`, `changeset_id`, and `binary` in its files.

### Added — terminal totals, colours (Sprint 11.02)

- **`+N −N` above the status bar**: lines added and removed by the changes applied to the session's workspace since it was opened — a sum of operations, not a `git diff`; `+0 −0` at first, kept at rest, across runs, compactions and cancellations; reset when a session is opened or resumed. Fed by `edit_applied` only, each change counted once (`agent_id` + call, or ChangeSet): successful `Write`, `Edit`, `MultiEdit`, `ApplyPatch` of the session agent; a sub-agent's in the same workspace (foreground or background); a ChangeSet once applied (from `/changes … apply` or the `AgentControl` tool), never its worktree writes, nor a conflicting, refused, repeated or discarded apply. Denials, refusals, failed writes and previews never count; binary files add no lines; events of another session are ignored. Counts are those of the complete diff. A hook that rewrites a call's input counts the change actually written.
- **Colours**: pale green (152, 205, 170) for added lines and `+N`, pale red (224, 153, 153) for removed lines and `−N`, shared by diffs, edit results, approval previews and the totals (separate spans; headers, hunks and context keep their style; the signs stay without colours).

### Changed (Sprint 11.02)

- `cost unknown (no price)` reads `Not priced`.

## [0.4.7] — 2026-10-08

### Fixed — retries honour `Retry-After` (Sprint 11.0.1)

- **The server's delay reaches the retry loop.** `Retry-After` was kept for a 429 but never read by the runner, and dropped for every other status (a 503's included). `CerseiError::ProviderStatus` now keeps it too (`retry_after` field), with `CerseiError::retry_after()` and `http_status()`. Whether an error is retried is unchanged: the header never makes an error retryable (quota exhausted, 4xx, authentication stay definitive).
- **HTTP dates are understood.** `parse_retry_after` reads delta-seconds (digits only; `0` is a valid zero) and HTTP dates (via `httpdate`, already in the lock file), measured from one reference time (`retry_after_at` takes it explicitly); a past date is a zero delay, an invalid value is absent.
- **One delay, used everywhere.** Each retry waits `max(local backoff, Retry-After)`: the backoff is unchanged (1, 2, 4, 8, 16 s, capped at 30 s, plus its jitter); a shorter, zero or past server delay keeps it, a longer one is honoured without a cap. The wait (cancellable), the notice and the log use that same value. Five retries after the first call (six calls at most), as before.
- **Notices say what happened, in milliseconds.** One message on every channel: `Rate limited (HTTP 429)` only for 429, `Service unavailable (HTTP 503)`, `Provider overloaded (HTTP 529)`, `Gateway timeout (HTTP 504)`, `Temporary provider error (HTTP 502)`, `Temporary connection error` (no status invented) — then `Retrying in 12000 ms... (retry 1/5)`. The retry log carries the class, the retry number and the delay, no longer the error's text.

### Changed — contract (Sprint 11.0.1)

- `CerseiError::ProviderStatus` gains `retry_after: Option<Duration>`: code that builds the variant literally adds it (`None`), or uses `CerseiError::from_http_status`; patterns without `..` add it. Its display is unchanged.

### Added — projects, history by project, terminal links (Sprint 11)

- **`--workspace <dir>`** (alias `--cd`, global, before or after the command) opens a project folder. Resolved without a shell: `~` and `~/…` expanded (also quoted, spaces kept; not `~user` nor variables), relative to the launch folder, canonical, a readable folder — a missing folder or a file is refused before any session, provider or request. Given twice it is refused. Bricks no longer changes its process's current folder; `--providers` is relative to the launch folder. The status bar ends with the project (`⌂ ~/…`, shortened with a mark of the full path when long).
- **The session's project follows the session.** `ProjectContext` and `ProjectLoader` (engine): a session opens with the project of its own folder — `bricks.toml`, rules, system prompt, long-term memory, profiles, CodeScout settings — read after the session is resolved (a broken `bricks.toml` in the launch folder does not block resuming another project's session). `resume <id>`, `/resume` and `run --session` prepare everything before switching; on error the open session is untouched. Approvals "for the session" do not carry over; the previous session's background jobs stop and never report as the new session. The interface follows `session_opened`: folder, completion index, previews. Embedders without a loader keep their injected configuration.
- **History by project.** `bricks sessions [--json]` lists the project's sessions (same selection in text and JSON; no prose on stdout in JSON); `--all` lists every one. Matching by canonical path identity (a symbolic alias matches; not a string prefix; a sub-folder or a worktree is another project). Older sessions and deleted folders appear with `--all`, said so. Listing needs no provider or key and changes nothing. Engine: `SessionScope`, `list_sessions_in`, `Controller::sessions_in`, `same_workspace`.
- **Session picker** (`bricks resume`, `/sessions`, `/resume`): this project's sessions, **Tab** for all projects; distinct empty states.
- **Clickable web links (OSC 8)** in the interface, `--hyperlinks auto|always|never` (default `auto`: recognised terminals only, off under tmux/screen). The destination is carried through Markdown parsing, styling and wrapping; Ratatui places the text, then each run of link cells becomes one forced-width cell, cut at style changes. Only `http`/`https` URLs with a host, serialised to printable ASCII, are activated; other destinations stay text, control characters neutralised. The address stays written next to the text. Nothing is stored or sent as JSONL; `bricks run` never writes OSC 8.

### Changed (Sprint 11)

- `bricks sessions --json` now lists the project's sessions; `bricks sessions --all --json` gives the former global listing.

## [0.4.6] — 2026-10-07

### Added — concurrent, background and isolated sub-agents (Sprint 10.5)

- **`Agents`**: several sub-agents in one call (`[agents] max_batch`), run concurrently within the session's `max_concurrent` slots; every entry validated before anything starts; results in the order asked; `fail_fast` cancels the siblings, otherwise one failure leaves the others' results; a full queue or an admission timeout is that child's own `failed` result (`AgentQueueFull`, `AgentAdmissionTimeout`). Each `agent_spawned` carries the parent's `tool_call_id` (per call, from the call's own context — no shared mutable state) and its `batch_index`.
- **Bounded recursion**: sub-agents have `Agent`, `Agents` and `AgentControl` up to `max_depth`, with `max_total_per_run` descendants per top-level run; refusals (`AgentDepthExceeded`, `AgentTotalExceeded`) before any provider is built. An agent waiting for its children releases its slot and its writer admission (`ActivityLease`): a parent → child → grandchild chain completes with `max_concurrent = 1`.
- **Background agents** (`background: true`): a handle at once; the child survives the parent's turn, its events reach the session stream, its result is kept without waking the parent. **`AgentControl`**: `list`, `status`, `result` (idempotent, no model call), `wait` (bounded; a timeout cancels nothing), `cancel` (with descendants), `inspect_changes`, `apply_changes`, `discard_changes`; per-action permission levels; a sub-agent sees only its descendants.
- **Worktree isolation** (`isolation: auto` picks it when writers would coexist): `git worktree add -b bricks/<run>/<agent>` in the session's files, user hooks off, from a **snapshot of the parent's effective state** (tracked changes, deletions, modes, binary, untracked non-ignored files) taken without touching the parent's index or files — no stash, reset, checkout, commit, nor any commit required first. **ChangeSets** are diffed from the worktree's baseline (inherited changes are not the child's), applied only on request with `git apply --check` then `git apply` (all or nothing), a conflict writes nothing (`WorktreeConflict`), discard removes only what the manifest says the runtime created and deletes a branch only if it still points at its base. Clear refusals: not a repository, no commit, submodules, collisions, snapshot limits, git < 2.17.
- **Jobs**: `Bash` with `background: true` returns a `job_id`; the **`Job`** tool (`list`, `status`, `output`, `wait`, `stop`) is scoped to the owning agent; quota `[background] max_jobs` checked before anything starts; stop is TERM to the group and tree, grace, KILL, idempotent; memory bounded per stream and per line (cut lines marked), raw logs capped and kept in the session's files; a sub-agent's jobs stop when it ends, none outlives the session. `[background]` in `bricks.toml`.
- **Profile skills** loaded into the child's system prompt (every request; within `skills_max_bytes`) and reported in the result (`loaded`, `truncated`, `missing`, `invalid`, `skipped`, with source and revision).
- **Usage**: `run_usage` (own, descendants, total; `final_total: false` with `pending_agents` while background descendants run, then final), each agent counted once.
- **Headless drain**: after the answer, background children are waited for up to `[agents] background_drain_ms`, then cancelled; exit code **6** if any did not complete.
- **Recovery**: instances recorded in `<session files>/agents/manifest.json`; on reopening, unfinished ones are marked `interrupted` — nothing replayed, no old pid touched.
- **Events and commands** (additive, JSONL schema stays 4): `changes_ready`, `changes_updated`, `run_usage`, `agent_control_result`, `job_started`, `job_output`, `job_finished`; states `queued`, `interrupted`; new optional fields on `agent_spawned` / `agent_finished`; command `agent_control`. Terminal interface: `/agents running|status|result|cancel`, `/changes <id> [inspect|apply|discard]`, `/jobs [stop <id>]`.
- The legacy `delegate` / `run_batch` paths count against the same slots and totals.

### Fixed (Sprint 10.5)

- `Write`, `Edit`, `MultiEdit` and `NotebookEdit` resolved a relative path against the process's current directory instead of the agent's working directory (`Read` already did): a sub-agent in a worktree would have written into the parent's tree.
- `ExitWorktree` no longer forces the removal and refuses the runtime's own worktrees.
- An unfinished output line of a shell command was buffered without bound; it is now cut at `max_line_bytes` (marked).

### Added — native sub-agents and profiles (Sprint 10.1)

- **`Agent` tool, native**: registered in the session agent of the CLI and the controller (`[agents] enabled`, default on), at the `execute` permission level. Delegates one self-contained task to a sub-agent in the **foreground**, with a fresh context (the task and an explicit, bounded `context`; never the parent's conversation, recalled memory or secrets), the parent's approval policy and broker (approvals carry the sub-agent's `agent_id`), the parent's tools rebuilt from its factory (never `tools::all()`, never a delegation tool), its own shell, and the workspace's CodeScout engine, web context and MCP connections. Returns a compact `AgentResult`: status (`completed`, `incomplete`, `failed`, `cancelled`), bounded answer, files changed by applied changes, commands actually run with their outcome, warnings, usage, duration, and a reference to the stored transcript. `AgentProfiles` lists and searches profiles (paged). See `docs/agents.md`.
- **Profiles** in Markdown with a YAML frontmatter, parsed with `serde-saphyr` under a budget (depth, nodes, aliases, scalar bytes), unknown and duplicate keys refused, no tag execution or interpolation. Registry `<workspace>/.bricks/agents` > `~/.bricks/agents` > built-in, with shadowing shown, localized diagnostics, an invalid or duplicated profile never replaced silently, reload without changing running sub-agents. Seven built-in profiles (`orchestrateur`, `web_searcher`, `inspecteur`, `backend_coder`, `frontend_coder`, `testeur`, `redactor`), no business quota or whitelist; two documented examples, loaded by a test.
- **`AgentSpawner`**: validation before any provider, shell or request (empty or invisible task, unknown profile, model, reasoning, limits, unknown fields, `background`/`worktree` → `not available yet` in 10.1, lifted in 10.5, depth, cancelled run); model and reasoning resolved request > profile > parent > default, with free reasoning ids, explicit aliases (`[agents.reasoning_aliases]`), `auto` only through `[agents] auto_model`, inconsistent explicit overrides refused, profile preferences degraded with a warning; instances with one terminal transition; writer admission per workspace (a sub-agent holds it for its run, other writing calls wait; the delegation call holds nothing, so no deadlock); the child runs in a task the parent's run supervises — a cancelled run waits (bounded) for the child's cleanup instead of dropping it.
- **Events** `agent_spawned`, `agent_state`, `agent_tool_started` / `agent_tool_finished`, `agent_finished`, `agent_profiles`; `approval_requested.agent_id`; commands `list_agent_profiles`, `reload_agent_profiles`; `/agents` in the terminal interface, sub-agent cells with their tool lines; headless text lines. JSONL schema 4.
- `[agents]` in `bricks.toml`.

### Changed — durations in milliseconds

- Durations shown to people are in milliseconds everywhere: tool lines (`Grep process_payment  12 ms`, even at `0 ms`), run summaries (`done in 12345 ms`, never switched to seconds), tool detail, headless text (`✓ Grep  12 ms`), tool report headers (`Succès (420 ms)` — a 3 ms call used to read `0.00s`), shell progress, readiness and background job status. `<1 ms` for a measured duration under a millisecond; a replayed call has no duration rather than an invented `0`; running calls show the elapsed time measured by the interface. `cersei_types::duration`. Stored `duration_ms` values and configured timeouts are unchanged.

### Fixed

- The runner's shell progress relay kept a strong reference to the run's event stream in the agent's extensions, so the stream could outlive its run; it is now weak.

### Changed — contract

- `cersei_tools::permissions::PermissionRequest` and `control::ApprovalRequest` gain `agent_id`. The system prompt's sub-agent section no longer claims sub-agents run in parallel.
- `Agent::adopt_mcp` (share another agent's MCP connections without closing them).

## [0.3.6] — 2026-10-07

### Added — shared code understanding engine (`bricks-semantic`, Sprint 9)

- **`bricks-semantic` crate**: one `SemanticEngine` per workspace (`SemanticRegistry`; a sub-folder shares it, a worktree or another checkout gets its own), `query(CodeQuery, &Requester) -> CodeResponse`. Intents `auto` (deterministic, explained routing), `text_search`, `find_symbol`, `definition`, `references`, `understand`, `diagnostics`; targets by position, offset, item id or file; scope, limits capped by configuration, context policy, `compact`/`normal`/`deep`. Items carry exact ranges and the content revision, with certainty (`confirmed`/`syntactic`/`textual`), freshness and a ranking score kept apart, plus provenance; homonyms are listed as `ambiguous`, never chosen silently. See `docs/semantic.md`.
- **9A, without any server**: lexical search on ripgrep's libraries (literal, regex, extensions, exclusions, ignore rules; files and buffers searched alike; parallel, deterministic, bounded by files, matches, size and time, with omissions reported); `PositionMapper` (bytes, lines, Tree-sitter points, LSP positions in UTF-8/16/32; CRLF, lone CR, emoji, invalid positions); views (disk, private buffers, isolated previews) and per-query snapshots, with files changed during a query flagged stale; Tree-sitter scopes, definitions and signatures (Rust, TypeScript, TSX, JavaScript via TSX, Python, Go), per-thread parsers, content-addressed tree cache with incremental reparse, line fallback for other languages; budgeted context (estimated with the Context Manager's heuristic, upper bound kept under the budget, explicit excerpts).
- **9B, optional language servers**: shared instances per (server, project root), single start under concurrency, per-document versioned synchronization, bounded restarts, idle shutdown, concurrency limit, open-document LRU; document symbols (outline), workspace symbols (on a running server), definition, references, hover and diagnostics with explicit fallbacks. Diagnostics are `analyzed`, `outdated`, `pending` or `unavailable` (no fixed delay), and compared with the previous analyzed version of the file. Servers are never installed by Bricks.
- **9C**: `CodeScout` tool (in `cersei_tools::filesystem()`), sharing the engine with sub-agents and the controller; prompt guidance only when it is registered (no quota, no relaunch); compiler locations (`cargo --message-format=json` preferred, text parsing marked heuristic) feed `understand` without running anything. Controller command `search` / event `search_results` and `/search` in the terminal interface (text only, never starts a server). Cached answers dropped after every non-read-only tool call. `[semantic]` in `bricks.toml`.
- **Indexing servers**: `$/progress` and `experimental/serverStatus` are followed; a query waits (bounded) for a server that is indexing, else reports `partial` and adds syntax/text results when the server answered nothing (found by the `--lsp` benchmark: rust-analyzer answered "no reference" before indexing and the result said `complete`). Re-exports listed by `workspace/symbol` are references, not definitions.
- **Benchmark** `cersei-tools` example `semantic_bench` with `bench/semantic/corpus.json`: on this repository, one call instead of 2–4 and 97–98 % fewer estimated tokens than Grep + Read, equal recall; references exact with rust-analyzer (precision 1.00 vs 0.25–0.40), text mentions without; latency regression when cold (up to ~4×), close warm. Tested against a real rust-analyzer 1.98.1. Numbers and limits in `docs/semantic.md`.

### Changed — `cersei-lsp`

- Capabilities from `initialize` kept (`ServerCaps`: position encoding, sync kind, providers, pull diagnostics); UTF-8 offered besides UTF-16.
- Server requests (`workspace/configuration`, `client/registerCapability`…) are answered; they were parsed as responses (ids could collide with pending requests) or dropped when their id was a string.
- `LspClient::request` with timeout and cancellation (`$/cancelRequest`); pending requests fail at once when the server exits (`LspError::ServerExited`; new variants `Cancelled`, `Unsupported`); `did_open`, versioned `did_change_full`, `did_save`, `did_close`; `connect` for in-process servers; the end of the server's stderr explains a failed start.
- Diagnostics kept with their version and arrival order; `wait_diagnostics`, `pull_diagnostics`. `LspManager::diagnostics` no longer sleeps 200 ms and returns whatever is there: it waits for the version sent (`diagnostics_with_state`); the `LSP` tool says when diagnostics are outdated or pending. `LspManager` resends a file whose content changed (it used to keep the first version forever).
- URIs percent-encoded and normalized (paths with spaces); `LocationLink` results accepted.
- `mock` feature: a scripted in-process server for tests.

### Changed — contract

- JSONL schema 3 (`search` command, `search_results` event).
- `Grep` uses the engine's walker (same ignore rules).

### Fixed — reliable stop and bounded sub-agents

- **A final answer ends the run.** Removed the relaunches that made agents over-zealous: the F-08 "you answered without using any tools" message with a forced `tool_choice: required` (every session, sub-agents and CLI included); the "read at least 8–10 source files" depth relaunch after an early answer; the "read at least 10 files" hint appended to prompts containing words such as *index* or *summary*. The benchmark verification nudges remain, only in `benchmark_mode` and bounded.
- **The response's content decides, not `finish_reason`.** Tool calls are run whatever stop reason came with them; a "tool use" stop without a call is an answer, never an empty round; calls in an answer cut by the output limit are answered as not run (the history stays valid) and the continuation says why.
- **Explicit termination.** `AgentOutput::termination` (`Completed`, `MaxTurns`, `OutputTruncated`, `NoProgress`, `ContentFiltered`, `EmptyResponse`); `max_turns = N` now allows exactly `N` generation turns (the counter used to report `N + 1`), and continuations count against it. `run_finished` gains `outcome = incomplete` and `termination`, and reports the turns that ran for failed and cancelled runs too (was 0); JSONL schema 2; `bricks run` exits with 5 for an incomplete run.
- **Repetition is judged on calls and results.** The old detector compared tool names only, so twelve different reads looked like a loop and an `[A, B]` cycle was flagged even without errors. A round that exactly repeats one of the last four (same calls, arguments and tool output) counts; the model is told once after 3 such rounds, the run stops after 5.
- **Sub-agents.** `Agent` and `delegate` refuse an empty or blank task (Unicode spaces and zero-width characters included), an impossible `max_turns` or concurrency, and a `model` override that was silently ignored, before building a provider; a batch with one invalid task starts nothing. Children get the parent's permission policy (they had `AllowAll`; `run_batch` without one is read-only), at most the tools they were given (an empty or unrebuildable set no longer falls back to the standard tools), never `Agent` or `delegate` (on both paths, whatever the factories return), depth from the context, the parent's working directory (`delegate` workspaces must be inside it), and the parent run's cancellation: no delegation starts after a cancel, running children stop and their shells close. A child stopped at a limit, cancelled, failed or with an empty answer is reported as such with its partial answer, never as a success.
- **Empty input.** A run with a blank prompt and no attachment is refused (`CerseiError::InvalidInput`) before anything is sent; an attachment-only prompt no longer carries an empty text block.
- **System prompt.** A "Scope and stopping" section (do the request, verify what matters, then stop; no minimum of reads, tools or delegations) replaces "just investigate", "never stop at surface-level answers", "always verify with tools" and "use TodoWrite very frequently"; capabilities are no longer advertised for tools that are not registered; the TodoWrite reminder reads the list TodoWrite writes to.
- **DeepSeek thinking mode (chat completions, `reasoning_field`).** The reasoning is sent back on every assistant turn, not only on tool-call turns: with tools, DeepSeek requires all earlier turns' `reasoning_content`.

### Fixed — performance regressions found by re-running the benchmarks

- **Bash: 1 s per command on macOS.** The kernel never reports the end of the output FIFOs, so each command waited two 500 ms drain windows (1 051 ms per `echo`). When nothing the command started is left, the output is now read until it is quiet (15 ms) instead: 37.6 ms per command. Regression test `a_quick_command_does_not_wait_for_the_output_window`.
- **Agent construction: 29 ms and 2.3 MB per agent.** The built-in compression rules were re-parsed from TOML for every agent built with the default configuration. They are now parsed once per process and shared (`RuleSet::builtin_shared`): 33.9 µs and 71.8 KB per agent.
- Stress suites (`crates/cersei/examples/stress_*`): expectations brought up to date (tool counts, compaction prompt wording, removed tool results, a stub model window too small for today's tool definitions); all 258 checks pass. `orchestration() = 3 tools` already failed in Cersei 0.1.6.

### Added — benchmarks

- `cersei-memory` example `memory_bench` (the README's Memory I/O table on the current API) and `scripts/bench_cli.py` (startup, binary size and peak RSS of `bricks` vs other agent CLIs; `--version` only). README rewritten with the re-measured figures and what was not re-run.

### Added — `bricks` CLI, command/event contract and terminal interface

- **Engine contract `cersei_agent::control`**: typed `Command`s (submit, cancel, set_model, compact, clear_context, resume, approve) and versioned `Envelope`s (schema 1, `session_id`, `run_id`, contiguous `seq`, `type`) with one `run_finished` per run (succeeded, failed, cancelled) and memory maintenance as its own phase after it. A `Controller` (the narrow entry point frontends use) and an `EventQueue`: bounded, deltas merged when the consumer is slow, nothing else dropped, cancellation never blocked by the consumer. Rules during a run: cancel immediate; model change applies from the next turn; submit/resume/compact/clear refused. See `docs/cli.md`.
- **Approvals** (`control::ApprovalGate`, `[permissions]` in `bricks.toml`): allow / ask / deny by tool then by permission level; requests carry a change preview (`Tool::preview`, implemented by Write, Edit, MultiEdit, ApplyPatch: files, diff, SHA-256 before); a change approved for a state that changed meanwhile is recomputed and asked again, never applied blindly; without anyone to ask, the run stops with `approval_required`. `edit_applied` reports what was written.
- **Sessions**: `session.json` (working directory, model, reasoning profile, memory space, title) next to each session; resume refuses a vanished directory or a model no longer configured instead of substituting one; `control::list_sessions`.
- **Attachments**: prompt blocks text / file / folder / image converted at submission (file contents captured with their SHA-256; folders as bounded listings; images checked by the provider against the model's capabilities). `Agent::run_input`, `UserInput`.
- **`bricks` binary (`crates/bricks-cli`)**: `bricks`, `bricks tui`, `bricks run` (positional prompt, `-`/stdin, `--stdin`, `--file`, `--image`, `--json`, `--non-interactive`, `--session`), `bricks sessions`, `bricks resume [id]`; documented exit codes (0, 1, 2, 3 approval needed, 4 memory maintenance failed, 130 cancelled); Ctrl+C cancels.
- **Terminal interface (`crates/bricks-tui`)**: inline viewport (transcript kept in the terminal scrollback), composer (graphemes, display widths, selection, history, bracketed paste never submits, typed attachments), `@` mentions (ignore + nucleo, refreshed index), declarative `/` commands, approval with diff, inspectors and pickers on the alternate screen (model/profile, sessions, memory, context, cost, tools, MCP, config, diff), Markdown rendering, terminal restored on exit, error and panic.
- `control::scripted` (scripted model and catalog, for tests and demonstrations), `scripts/demo_model.py` (local demo server, no key).
- `[agent]` and `[permissions]` sections of `bricks.toml`; `[memory]` gains `enabled`, `path`, `embeddings`, `extractor_model`.

### Changed

- `Agent`: the model and reasoning profile can change at runtime (`set_model`, from the next turn); cancellation is per run (`cancel` no longer poisons later runs); `AgentStream::cancel` now reaches the run (its control channel was never read); a failed or cancelled run answers its unanswered tool calls ("interrupted") and stores the session; `save_session`, `clear_context`, `maintain_memory`, `model_label`, `reasoning_profile`, `run_cancellation`.
- `cersei_memory::LongTermMemory`: `record_turns` only stores durably; the new `maintain` does extraction and embeddings (cancellable, resumable). After a run, the agent reports `MemoryMaintenanceStarted/Finished`; `MemoryRecalled` replaces the recall status note.
- `PermissionRequest` gains `preview`; `Tool` gains `preview` (default `None`); `cersei_types::Usage` derives `PartialEq`.
- `Agent::run` no longer keeps an undrained event channel (it could block once 512 events were buffered).

### Fixed

- Structured memory: reads that bypass Grafeo snapshots (adjacency, single properties, text search) wait for an open transaction to commit, so recall never sees uncommitted links; extraction no longer holds the memory's write lock across the model call, and embedding passes can be cancelled.


### Added — structured long-term memory and hybrid recall

- **`cersei-memory` feature `structured`** (`docs/memory.md`): sourced, immutable **episodes**; **entities** resolved by name/alias within an explicit space (`user:`, `project:`, `space:`); temporal **facts** with evidence (episode + verbatim quote), origin (user statement, assistant proposal, tool output, summary), status (active, proposed, superseded, contested, retracted, unsupported) and two time axes (validity: stated `valid_from`/`valid_until`, `asserted_at`; knowledge: `recorded_at`, `superseded_at`, `retracted_at`). Explicit changes on exclusive predicates replace the old value (`new SUPERSEDES old`); silent conflicts are kept as `contested` (`CONTRADICTS`). Current facts and "valid at T as known at K" are views of the same records. Injectable clock.
- **Resumable ingestion**: record → extract (provider-based `LlmExtractor`, strict validation, limits, timeout, cancellation; output cached before applying) → apply in one Grafeo transaction → embed. Failed extraction or embedding never loses the episode; a restart never calls the model again for a cached extraction; re-processing never duplicates facts.
- **Hybrid recall**: vector (USearch projection rebuilt from the store, filtered during the search), lexical (Grafeo BM25), bounded 1–2-hop relational expansion, weighted RRF (`w/(60+rank)`, configurable), dedup that keeps differing values/dates/negations apart, rendering with sources within a token budget, per-stage timings.
- **Agent**: `AgentBuilder::long_term_memory(Arc<dyn LongTermMemory>)` and `memory_recall_tokens`; recall is added to the system prompt of each run (capped at a tenth of the prompt budget) and each exchange is recorded. Trait `cersei_memory::LongTermMemory` (no feature needed).
- **`cersei-embeddings`**: `EmbeddingProvider::model_id` (vector space identity, overridden by OpenAI/Gemini), `VectorIndex::{search_filtered, remove, contains, capacity}`, `HashingEmbeddings` (local, deterministic, lexical — tests and offline benchmarks).
- **LongMemEval**: `longmem-recall` binary (evidence `recall_any@K`/`recall_all@K` per question type, latencies; modes legacy / vector / vector-lexical / hybrid; never ingests answers or evidence annotations) and `structured-vector` / `structured-hybrid` configurations in `longmem-bench`.
- **`[memory]` section** of `bricks.toml` (`MemoryConfig::from_bricks_toml`), documented in `docs/bricks.example.toml`.

### Changed

- **Graph schema v3** (migration from v2: legacy `:Memory` nodes become searchable `space:legacy` episodes; `:Memory` nodes kept).
- **`GraphMemory` queries are parameterised** (they spliced hand-escaped content into GQL). Behaviour unchanged, including `recall` matching the whole query as one substring — the cause of the empty retrieval behind the 6.6 % LongMemEval figure (see `docs/memory.md`).
- `cersei-memory`'s Grafeo dependency enables `text-index`; the `structured` feature also depends directly on `grafeo-core` (same locked version as `grafeo`) for adjacency reads.
- The structured memory writes with Grafeo's typed node/edge API (GQL writes grew superlinearly in 0.5.43), batches episode recording (`record_all`: Grafeo transactions cost time in the size of the graph), and reads expansion neighbours from adjacency lists.

### Fixed

- `cersei-web`: `SearchError` and `PageError` failed to serialise (internally tagged newtype variants holding strings), which made `WebSearch` panic when a network error occurred. They are now adjacently tagged (`{"kind": …, "detail": …}`).

### Changed — web search, page reading and MCP

- **New crate `cersei-web`**: search → bounded concurrent downloads → Markdown extraction → structural passages → BM25 → citable passages (`docs/web.md`). DuckDuckGo HTML by default (no key); Brave, Tavily and Exa through `bricks.toml` `[web.search]` (provider, ordered fallback, `api_key_env`), with a visible, bounded cascade; challenge pages, unknown layouts, refused keys, quotas and rate limits are told apart. Network policy: private/local/reserved addresses refused on every hop and every DNS answer (explicit `allow_private` exceptions). Decoded-byte limits per page and per call, per-host concurrency, deterministic per-URL results, cancellation.
- **`WebSearch` rewritten** on `cersei-web`: results (`S1`…), pages read (`D1`…), verbatim passages with section and character range, every provider attempt shown. The former behaviour (Brave only, key from `CERSEI_SEARCH_API_KEY`, endpoint from `CERSEI_SEARCH_API_URL`) is gone: a provider is used only when `bricks.toml` names it.
- **`WebFetch` rewritten**: Markdown in document order, paged by Unicode characters (`offset`, `max_chars`, next offset, omissions stated), optional `question` for passages, `doc` to read a stored document, `format` `raw` / `json_summary`, `refresh`. A page is downloaded once per session; documents are stored with the session (restored and deleted with it). PDF/binary and JavaScript-only pages are reported, not shown.
- **Removed**: the `ExaSearch` tool (Exa is a `[web.search]` provider), `tool_primitives::http::fetch_html` and the `html2text` dependency. `cersei_tools::web()` returns `WebFetch` and `WebSearch`.
- **Compression**: `WebFetch`/`WebSearch` output is no longer reduced line by line (it is already paged and selected); over `max_output_chars` it is cut once on a line end.
- **`cersei-mcp` rebuilt on the official SDK** (`rmcp` 3.5): protocol 2026-07-28 (`server/discover`, per-request `_meta`, `resultType`, MRTR) with automatic fallback to the `initialize` handshake of 2025-11-25; stdio and Streamable HTTP (bounded HTTP client on the workspace's reqwest); progress tied to its request, idle and total timeouts, cancellation, `max_in_flight`, message size limit, no replay after a lost connection, stderr kept as a log (`docs/mcp.md`). No client capability is declared (the former `roots` declaration had no handler). The hand-written `jsonrpc`/`transport` modules are removed.
- **API changes (`cersei-mcp`)**: `McpServerConfig` gains `headers`, `cwd`, `protocol`, `limits`; `McpServerConfig::sse` is replaced by `::http` (the HTTP+SSE transport is refused); `McpManager::connect` returns the manager (failures in `statuses()`), `call(server, tool, args, progress)` returns `McpCallResult` (content blocks, structured content, `is_error`); errors are `McpError`.
- **Agent**: MCP servers given to `AgentBuilder::mcp_server` are now connected at the first run and their tools offered as `mcp__<server>__<tool>` (previously never connected); `Agent::close()` closes them. `AgentBuilder::web_config` and `BricksConfig::web` (`[web]`) configure the web tools.

### Changed — shell, tool results and file tools

- **Persistent shell**: one live bash per agent session, driven over a dedicated control channel (fds 198/199, framed NUL-separated replies with request id, status and cwd). The sentinel wrapper (`__ABSTRACT_STATE_7f2a9b__`, `cd` prefix / `pwd` suffix) and `ShellState`/`session_shell_state` are removed. cwd, variables, PATH, venvs, aliases and functions persist while the shell lives; a shell that ends is reported and replaced, never faked. stdin is empty unless `input` is given; the shell has no controlling terminal.
- **Supervised execution**: stdout/stderr read concurrently from the start (bounded memory, raw spilled to disk), incremental ANSI/CSI/OSC/progress cleaning across chunks, progress events (`AgentEvent::ToolProgress`), timeout (120 s default) with TERM→KILL of the command's process tree, partial output and verified termination; leftovers of finished commands are stopped; cancelling a turn interrupts the running command (the runner now watches the cancel token during tool execution). `Agent::close()` and `Drop for Agent` stop the session's processes.
- **Background tasks**: `background: true` (with `ready_pattern`/`ready_timeout`), `BashTaskStatus`, `BashTaskOutput` (cursor paging), `BashTaskStop`; tasks inherit a snapshot of the shell state, run in their own process group, are session-scoped and stopped with the session.
- **PowerShell** keeps one interpreter per session (loopback control channel), and separates PowerShell success, cmdlet errors and native exit codes. Not executed on Windows in this sprint (see `docs/shell.md` §9).
- **Tool results are structured**: `ToolResult::report` (`ToolReport`: status, real exit code or none, termination, duration, streams, suggestion, notes, data) and one rendering for every tool (`✓ [Bash] Succès (0.42s) — code 0`, `--- stdout ---`, …). Compression sees only the output. `Tool::permission_details` lets the shell show session aliases/functions to permission policies.
- **Edit/MultiEdit**: exact → normalised (line endings, trailing spaces, constant indentation shift; never inside strings or here-docs) → bounded relocation (blank lines, spacing outside quotes); different code is refused with candidates; replacements follow the file's indentation and line endings; the file is re-validated before an atomic write. `ReplaceError`/`EditError` changed accordingly.
- **ApplyPatch** rewritten: strict parsing (counts, quoted paths, timestamps, `/dev/null`, no-newline markers), exact context, explicit refusal of binary/rename/mode patches, path confinement (no `..`, absolute paths or symlinks), full pre-validation and rollback on write failure. `apply_patch::patch_targets` is shared with the read-before-edit guard.
- **Read**: `  12 | text` numbering, streaming count of the exact total, binary/UTF-16/non-UTF-8 detection, BOM handling, change-during-read detection; relative paths resolve against the agent's working directory.
- **Clippy**: `cargo clippy --workspace --all-targets -- -D warnings` passes. API renames for it: `MemoryType::from_str`, `MemoryCategory::from_str`, `OutputStyle::from_str` → `parse_name`; `HookAction::InjectMessage` now holds a `Box<Message>`; `FileHistory::record_*` take `&Path`.

### Changed — context management and compression

- **Context manager** (`cersei_agent::context`): three separate counters — `context_used` (occupation of the active context, with its provenance: `measured` from the server's `usage`, `counted`, `mixed` or `estimated`), `context_window` (the configured limits; no total invented when `context_window_tokens` is absent) and `total_tokens` (session consumption, cache reads/writes included once, compaction calls included, never decreasing). A `usage` is recorded with the model identity and the context version; responses kept in the history are counted from their own output minus reasoning the protocol does not re-send; rewriting the history, changing the instructions/tools or the model invalidates the measurement. `Agent::context_status()`, `AgentEvent::ContextUpdate`, and `ModelRequestStart.token_estimate` (previously always 0).
- **Budget before every request**: output reserve (bounded by the model's `max_output_tokens`, reasoning included) and a configurable margin; a pre-flight on large injections, near the limit or after a rewrite; a request that manifestly does not fit is compacted first and otherwise **not sent** (`CerseiError::ContextOverflow` + a `Status` event). An explicit "too long" refusal from the server (`CerseiError::is_context_overflow`) is compacted and retried at most `max_overflow_recoveries` times per turn.
- **Pre-flight counting** only when a model configures `[providers.models.token_counting]` (`anthropic_messages`, `responses`); refused for `chat_completions`. `Provider` gains `model_info()` and `count_request_tokens()`; the old `count_tokens()` (characters ÷ 4) is removed.
- **Compaction** rewritten: budgeted summary call, fixed headings (objective, constraints, decisions, files, actions, open problems, key facts), the user's messages kept verbatim, gain verified before anything is replaced, explicit outcomes (`Compacted`, `InsufficientGain`, `Failed`, `Skipped`) on `AgentEvent::CompactionResult`, bounded attempts. The silent snip fallback is no longer used by the runner. `Agent::raw_history()` and `Agent::compaction_snapshots()` keep everything; with a session memory they are stored as `<session>.raw` and `<session>.compaction-<n>`. `compact::compact_conversation` and `auto_compact_if_needed` are replaced by `compact::compact_history`.
- **Tool-output compression** (`cersei-compression`) rebuilt: diagnostic blocks found before any cut (the middle of a long log is no longer lost to head/tail truncation), commands recognised through wrappers and quoting rather than by prefix regex, rules for cargo test/clippy/audit, go test, bun, vite, turborepo, vitest, eslint, next build, yarn, npm, pnpm, uv, ruff, mypy, pytest, kubectl, terraform, gh, docker compose; structured `cargo --message-format=json` and `go test -json`; every reduced output names its saved original (`[bricks: …]` header, readable with `Read`). Error results are processed too (diagnostics first) instead of a blind head/tail cut; the hard cap applies at every level. `cap_tool_result` is removed; `apply_tool_result_budget` now returns whether it changed anything, and `apply_tool_result_budget_with` lets the placeholder name the saved original.
- **Rules are configuration**: `~/.bricks/rules/*.toml` and `[compression.filters]` in `bricks.toml`, with a documented priority, replacement and `disabled = true` by id, and precise diagnostics for invalid rules. The rule schema changed (`match = [{ program, subcommands }]` instead of `match_command`).
- **Tree-sitter skeletons** replace the regex/brace-counting body stripping (`aggressive` level): Rust, Python, TypeScript, JavaScript/TSX, Go; original line numbers kept; uncertain regions shown in full; a skeleton never counts as having read the file. **JSON summaries** with a separate envelope (shape, verbatim sample, omissions by JSON Pointer).
- **Durable originals**: with a session memory, saved tool outputs live in the session's files directory (`Memory::session_files_dir`, `JsonlMemory`: `<session>.files/`); raw history, snapshots and outputs survive a restore (snapshots are reloaded, numbering continues) and `JsonlMemory::delete` removes them with the session. `JsonlMemory::sessions()` no longer lists `<session>.raw` / `<session>.compaction-<n>` when the session exists (`cersei_memory::session_keys`).
- `tbench-agent` and `cersei-agentrl` load `BricksConfig` (`CerseiRunner::with_bricks_config`); `tbench-agent --compress` now defaults to `[compression] level`, else `minimal`.
- `bricks.toml` (`[context]`, `[compression]`) via `BricksConfig`; `AgentBuilder::context_policy`, `compression_config`, `compression_rules`, `raw_output_dir`, `bricks_config`. The `Bash` tool now separates stderr with a `--- stderr ---` line. See `docs/context.md`, `docs/compression.md`, `docs/bricks.example.toml`.

### Changed — providers are now configuration (breaking)

- **A single configuration file is the only catalogue of providers and models** (`~/.bricks/providers.toml`, or an explicit `.toml`/`.json` path that *replaces* it; `schema_version = 1`). Adding a provider, or a model compatible with a supported protocol, is a configuration change — no recompilation. Selection is explicit (`provider_id/model_id`); nothing is detected from a model name. Environment variables only resolve secrets the file references (`api_key_env`). See `docs/providers.md` and `docs/providers.example.toml`.
- **Three protocol adapters replace the per-vendor providers:** `chat_completions`, `responses`, `anthropic_messages` (request, response, streaming, tool calls, multi-turn history, stop reasons, usage). Reasoning blocks, signatures and opaque protocol items required when history is re-sent are preserved. Requests the model or adapter cannot carry (video, undeclared media, unsupported source kinds) are refused with an explicit error before sending — attachments are never dropped silently.
- **Reasoning profiles are user-defined** (id, label, parameters, JSON-Pointer removals) and merged into the request in a fixed order, with structural fields protected. The closed `EffortLevel` enum, `thinking_budget` and the per-model thinking/temperature gates are gone.
- **Prices are declared and exact** (USD per million tokens, cache read/write, named tariff variants). An absent price is *unknown*, never free; estimates say when they are partial. The built-in price table and `CostTracker::add_with_model` are removed. `Usage` gains `reasoning_tokens` (a subset of `output_tokens`) and `cost_estimate`.
- **Removed:** `Anthropic`, `OpenAi`, `Gemini`, `AnthropicVertex`, `Auth`/`AuthProvider`/`OAuthToken`, `from_model_string`, the built-in registry, `ProviderQuirks`, the Gemini schema dialect, `gcp_auth`, the OAuth and Gemini-vision examples. `cersei-embeddings` is unchanged.
- **Types:** `ContentBlock` gains `Audio`, `Video` and `ProtocolItem`; image/document sources gain `file_id`; `ContentBlock::media_bytes`/`from_path` now route audio and video to their own blocks (they used to become `Image` blocks). `CompletionRequest` gains `output_modalities`; `CerseiError` gains `Unsupported`.
- `tbench-agent` takes `--model provider_id/model_id` (required) and `--providers`; `longmem-bench` takes `provider_id/model_id` selections for the answerer, judge and extractor and `--embeddings openai|gemini`.

## [0.2.5] - 2026-06-29

### Changed

- **`Grep` is now native and in-process — no external `rg`/`grep` binary required.** The tool previously shelled out to ripgrep if `which` found it on `PATH`, else fell back to system `grep` (losing gitignore-awareness and speed). Since `rg` is rarely preinstalled, models that reached for it — directly or via the tool — failed on stock machines. `Grep` (and the `tool_primitives::search::grep` primitive) now use ripgrep's own library crates: [`ignore`](https://docs.rs/ignore) for a lock-free **parallel, gitignore-aware** recursive directory walk and [`grep`](https://docs.rs/grep) for the regex matcher/searcher. Behavior is now identical on every machine, and the tool description steers models to use `Grep` instead of running `rg` in Bash.
  - **Respects `.gitignore`/`.ignore` and skips hidden + binary files by default** (gitignore is honored even outside a checked-out git repo). Output is sorted by `(file, line)` for **deterministic** results.
  - **New `GrepOptions` fields** (`no_ignore`, `hidden`), both defaulting to today's behavior so `GrepOptions::default()` is unchanged. The `Grep` tool exposes `case_insensitive` and `hidden` in its input schema; `pattern`/`path`/`glob` and the 250-match cap are unchanged.
  - New workspace dependencies: `ignore` and `grep` (both Unlicense OR MIT). MSRV stays satisfied (these track Rust 1.85).
- Workspace bumped to **0.2.5** across every crate via `version.workspace = true`.

## [0.2.4] - 2026-06-28

### Added

- **New `MultiEdit` tool — apply several string replacements to one file atomically.** Weaker models bungle refactors that need many separate `Edit` calls (e.g. a variable rename touching several lines): each call re-reads stale context and the sequence drifts. `MultiEdit` takes an ordered `edits: [{old_string, new_string, replace_all?}]` list, applies them **sequentially in memory** (each edit sees the previous edit's result), and writes **all-or-nothing** — if any edit fails to match, the file is left untouched and the failing edit is named (`edit N of M`) with a corrective message. Every edit routes through the same tolerant replacer ladder as `Edit`, so it inherits whitespace/indentation tolerance and the destructive-match guard, plus the same input coercion (aliased field names, stringified booleans, missing `new_string` ⇒ deletion). Registered in `cersei_tools::filesystem()` (and thus `all()`/`coding()`).
  - For a single-pattern mass replace (rename `foo`→`bar` everywhere), `Edit`/`MultiEdit` with `replace_all: true` already does it in one call; `MultiEdit` adds the case of *several distinct* edits in one atomic call.

### Changed

- Workspace bumped to **0.2.4** across every crate via `version.workspace = true`.

## [0.2.3] - 2026-06-28

### Changed

- **The `Edit` tool now tolerates whitespace/indentation drift instead of requiring a byte-exact `old_string`.** Exact matching made edits brittle: weaker BYOK models (Qwen, DeepSeek, Gemini Flash) routinely drift on leading whitespace and indentation, so an otherwise-correct `old_string` failed with `old_string not found` and the model fell back to slow, error-prone `sed`/`cat` edits. `Edit` (and the `tool_primitives::fs::edit_file` primitive) now run a **replacer ladder**: exact match first, then line-trimmed, block-anchor, whitespace-normalized, and indentation-flexible strategies. Every strategy only ever yields a candidate that *actually exists in the file*, so a fuzzy match relaxes *how* the text is located, never *what* gets written.
  - **Destructive-match guard.** The line-based strategies require every line to match (after normalization); the one anchor-only strategy (block-anchor) is additionally gated by a similarity threshold so a coincidental first/last-line pair can't rewrite an unrelated block. Exact matches are still tried first, so genuine duplicates report `AmbiguousMatch` rather than being silently fuzzed.
  - **Input coercion + corrective errors.** The tool now accepts common near-miss field names (`path`/`old`/`new`), stringified booleans (`replace_all: "true"`), and a missing `new_string` (treated as a deletion). Failure messages now guide the model toward a fix (re-read the file; add context to disambiguate; set `replace_all`) instead of a bare error.
  - New primitive: `tool_primitives::replace::replace`.
- Workspace bumped to **0.2.3** across every crate via `version.workspace = true`.

## [0.2.2] - 2026-06-24

### Added

- **Multimodal input across the SDK.** Messages can now carry images, video, audio, and documents (PDF/text) — not just text — and the three first-party providers serialize them to each backend's native wire format. The provider-agnostic blocks (`ContentBlock::Image` / `ContentBlock::Document`) already existed; this release adds the high-level constructors to build them and teaches every provider to send them. Docs: [Multimodal Input](https://cersei.pacifio.dev/docs/multimodal).
  - **High-level constructors** in `cersei-types` (re-exported through `cersei::prelude`):
    - `ContentBlock::from_path(path)` — reads a local file, **auto-detects the MIME type** from magic bytes (PNG/JPEG/GIF/WebP/PDF/MP4/MOV/WebM/MP3/WAV/Ogg…) with an extension fallback, base64-encodes it, and routes to an `Image` block (image/video/audio) or a `Document` block (PDF/text).
    - `ContentBlock::image_bytes` / `image_base64` / `image_url`, and the `document_*` equivalents, plus `media_bytes(mime, bytes)` when you want to set the type explicitly.
    - `Message::user_with_files("caption", &paths)` and `Message::user_with_media(text, blocks)` for one-call multimodal user messages.
    - `MediaKind` + `detect_mime(bytes, path)` exported for callers that want the classifier directly.
  - **Provider serialization.** OpenAI now emits a typed `parts` array (`image_url` data-URLs for images, `file` parts for PDFs; video/audio are dropped since Chat Completions can't accept them). Gemini emits `inlineData` for base64 and `fileData`/`fileUri` for URLs — the same path for images, **video**, audio, and PDFs. Anthropic already round-tripped images and PDFs via direct serde (now covered by a wire-shape test).
  - **`Gemini` is now exported from the `cersei` facade** (`cersei::Gemini`, in the prelude) — previously only reachable via `cersei_provider::gemini::Gemini`.
  - **Examples.** `cersei/examples/multimodal.rs` (provider-agnostic) and `cersei/examples/gemini_vision_test.rs` (a live image-analysis smoke test).

### Changed

- Workspace bumped to **0.2.2** across every crate via `version.workspace = true`.
- **Gemini `thinking_budget` is now configurable** via `ProviderOptions` (mirrors the Anthropic provider). `gemini-2.5`+ models spend dynamic-thinking tokens out of `maxOutputTokens`, which silently truncates a small-budget response; `options.set("thinking_budget", 0)` disables thinking so the full budget goes to the visible answer. Surfaces as `generationConfig.thinkingConfig.thinkingBudget`.

## [0.2.1] - 2026-06-23

### Added

- **New crate: `cersei-workflows`.** A first-party workflow engine designed so an entire workflow round-trips to and from a visual builder (React + xyflow), powering the Atlas workflow UI. Feature-gated behind `workflows`; mirrors the Mastra DX (`createStep`/`createWorkflow`, `.then`, input/output JSON schemas, `.start()`/`.stream()`, shared state, workflows-as-steps, suspend/resume, status-discriminated run results). Docs: [Overview](https://cersei.pacifio.dev/docs/workflows-overview) · [API](https://cersei.pacifio.dev/docs/workflows-api) · [Cookbook](https://cersei.pacifio.dev/docs/workflows-cookbook).
  - **`ir`** — the serializable `WorkflowDef` (`nodes` + `edges`) is the single source of truth; the programmatic `WorkflowBuilder` and the UI emit the *same* IR (one IR, two front-ends), compiled once by `Workflow::compile`. Control flow covers sequential (`then`), parallel fan-out/`Join`, conditional `Branch`, `Loop` (`dowhile`/`dountil`/`foreach`), and `Map` reshape nodes, each with a clean xyflow mapping. `UiHints` carry x/y/label and are ignored at execution time, keeping the IR lossless across the React Flow boundary.
  - **`step`** — a `Step` trait mirroring `cersei_tools::Tool` (id + input/output JSON schema + async `execute`), plus first-party `FnStep`, `AgentStep` (wraps a `cersei_agent::Agent`), `ToolStep` (wraps any `Tool`), and `WorkflowStep` (nested workflows).
  - **`registry`** — a `StepRegistry` (mirrors AgentRL's `ToolRegistry`) maps step-ids to executable steps; UI JSON carries only references, the host supplies implementations, and `catalog()` feeds the builder palette.
  - **`condition`** — a small serde predicate enum over JSON-pointer paths (UI-editable, side-effect-free) instead of an opaque expression language, so branches render and edit structurally in the UI.
  - **`executor` / `events` / `store`** — a memoized recursive DAG walker; a `Serialize`-able `WorkflowEvent`/`WorkflowStream` for live UI status over SSE/WebSocket; and an in-memory `RunStore` backing snapshot-driven suspend/resume.
- The `workflows` feature is opt-in on the `cersei` facade (`cersei = { version = "0.2.1", features = ["workflows"] }`), re-exported as `cersei::workflows` with the common types in `cersei::prelude`.

### Changed

- Workspace bumped to **0.2.1** across every crate via `version.workspace = true`; the layout now includes `cersei-workflows`.

## [0.2.0] - 2026-06-19

### Added

- **New crate: `cersei-agentrl`.** AgentRL — a self-evolving orchestration layer on top of the Cersei agent SDK. Governs the run → fail → trace → plan → sandbox → promote → register loop via an `ExecutionGraph` and a dynamic `ToolRegistry`. Docs: [Overview](https://cersei.pacifio.dev/docs/agentrl-overview) · [API](https://cersei.pacifio.dev/docs/agentrl-api) · [Template DSL](https://cersei.pacifio.dev/docs/agentrl-template) · [Cookbook](https://cersei.pacifio.dev/docs/agentrl-cookbook).
  - **`graph` / `graph_reporter`** — build a failure-tracing DAG from agent events.
  - **`registry`** — a persisted, searchable database of agent-built tools.
  - **`orchestrator`** — the RL loop, written against the `AgentRlRunner` trait.
  - **`scrub`** — enforces the hard rule that no secrets land in persisted artifacts (traces, planned tools, registry rows).
- **New crate: `cersei-agentlang`.** The AgentTemplate DSL — a small functional language that LLMs and agents author and that executes on top of the Cersei runtime. Pipeline: `parse` → `Program` → `run_program` over an `EvalCtx`. Builtins (`io.*`, `net.*`, `agent.*`, `kv.*`) dispatch by tool name through the `ToolDispatch` trait, so the language is decoupled from the concrete tool set. Ships an LLM-facing `AGENTLANG_SPEC`.
- **New crate: `cersei-tbench`.** A purpose-built Terminal-Bench coding agent on the Cersei SDK + AgentRL, with its own task prompt.
- **`--agentrl` mode in abstract-cli** (`agentrl_run.rs`). Solves a single task with the self-evolving AgentRL orchestrator instead of a one-shot agent. Designed for headless / terminal-bench runs: resolves a provider from `--model`, builds a verifier that prefers a task-provided `run-tests.sh` and falls back to accepting the agent's result, runs the `Orchestrator` in the working directory, and prints a machine-readable result line compatible with the harbor adapter.
- **Anthropic via Google Vertex AI provider** (`cersei-provider::AnthropicVertex`, `vertex` registry id). Reuses the direct-Anthropic request body + SSE parser; differences are the Vertex `:streamRawPredict` URL, `Authorization: Bearer <gcp token>` auth, and the `"anthropic_version": "vertex-2023-10-16"` body field. Auth resolves in order from a self-refreshing service-account JSON (via `gcp_auth`, required for long runs since GCP tokens expire ~1h), a pre-minted `VERTEX_ACCESS_TOKEN`, or `gcloud auth print-access-token`.
- **First-class custom OpenAI-compatible endpoints.** Any OpenAI-compatible provider's base URL can now be overridden with `<PROVIDER>_BASE_URL` (e.g. `OPENAI_BASE_URL`, `DEEPSEEK_BASE_URL`, `OPENROUTER_BASE_URL`); `OPENAI_API_BASE` is accepted as a legacy alias for `openai`. `ProviderEntry::resolved_api_base()` performs the lookup. When an override is in effect, a redacted final-target diagnostic (`provider=… host=… model=… key_present=…`) is printed before the first request so a misrouted endpoint is obvious instead of surfacing as an opaque `401`. Closes [#17](https://github.com/pacifio/cersei/issues/17) (item 2).
- **`deepseek-reasoner`** added to the DeepSeek provider registry (64K context, thinking capabilities) alongside `deepseek-chat` and `deepseek-coder`. Docs updated with a minimal `--provider deepseek` working example and the `<PROVIDER>_BASE_URL` override table.

### Changed

- Workspace bumped to **0.2.0** across every crate via `version.workspace = true` (18-crate layout now includes `cersei-agentrl`, `cersei-agentlang`, `cersei-tbench`).
- `ProviderEntry::api_key_from_env()` now **trims** the env value before use — launchers that reconstruct a key via command substitution (`KEY="$(…)"`) commonly leave a trailing newline, which previously produced a confusing `401` instead of a clean error. Whitespace-only values are treated as absent. Addresses [#17](https://github.com/pacifio/cersei/issues/17) (item 1).
- The OpenAI-compatible router path resolves its base URL through `resolved_api_base()` instead of the hardcoded registry default, so `--provider deepseek` always targets `api.deepseek.com` and never silently falls back to `api.openai.com`.

### Fixed

- **UTF-8 panic on multibyte input in the TUI** (`assertion failed: self.is_char_boundary(idx)`). The text-input cursor in `tui/event_loop.rs` tracked `cursor_pos` as a byte index but advanced it by one *character* per keypress, so typing a multibyte character (Chinese / Japanese / Korean, etc.) and pressing a key landed the cursor mid-character and panicked on the next insert. Char insert, Backspace, Left, and Right now move by the char's UTF-8 byte length. Fixes [#14](https://github.com/pacifio/cersei/issues/14).
- **UTF-8 panic on accented characters** (`byte index … is not a char boundary; it is inside 'à'`). Six byte-slice truncators across the CLI and agent runner (`render::truncate`, `event_loop::truncate`, `graph::truncate_label`, `messages::wrap_text`, `widgets/input` wrap, and `runner` content head/tail cuts) could split a multibyte UTF-8 sequence. The `truncate*` helpers are now char-based; the word-wrap and content-cut paths floor/ceil their slice points to char boundaries with a forward-progress guard so a single wide char can't loop. Fixes [#17](https://github.com/pacifio/cersei/issues/17) (item 3).

## [0.1.9] - 2026-05-17

### Added

- **New crate: `cersei-vms`.** Sandbox & VM isolation layer for coding agents. Lets the agent run shell commands and edit files inside isolated environments instead of on the host, and lets multiple parallel agents run in separate sandboxes that share state through host-mediated primitives. 15th workspace crate. Docs: [VMs Overview](https://cersei.pacifio.dev/docs/vms-overview) · [API](https://cersei.pacifio.dev/docs/vms-api) · [Cookbook](https://cersei.pacifio.dev/docs/vms-cookbook).
  - **Core traits** in `cersei_vms::runtime` — `SandboxRuntime`, `Sandbox`, plus `Commands` and `Filesystem` per-sandbox surfaces. Mirrors E2B's `Sandbox` / `Commands` / `Filesystem` shape almost 1:1, so existing E2B-based mental models port directly.
  - **Backends.** `LocalProcessRuntime` (always-on, no isolation — for tests and `--sandbox local`) and `DockerRuntime` (real container isolation via the local `docker` CLI; feature `backend-docker`, default-on). Phase 1 ships without `bollard` / HTTP-over-UDS — we shell out, which works identically on macOS, Linux, and Windows wherever Docker is installed.
  - **Cross-sandbox primitives** in `cersei_vms::primitives`. `Volume` registry (host-side dirs bind-mounted into N sandboxes), `Mailbox` (broadcast pub/sub by topic, backed by `tokio::sync::broadcast`), `KvStore` (DashMap + optional journal file, versioned CAS via `cas(key, expected_version, value)`). All three are reachable from sandboxes through a host-side broker; sandboxes never need direct network links between each other.
  - **First-party snapshots.** `Sandbox::snapshot() -> SnapshotId` and `SandboxRuntime::restore(&SnapshotId) -> SandboxHandle`. Docker implementation = `docker commit` for FS state + a JSON `SnapshotManifest` (env, mounts, mailbox topics, KV checkpoint) in `~/.cersei/vms/snapshots/`. Local implementation = directory copy. Snapshots survive process restart.
  - **`cersei-envd` binary** (feature `envd`, default-on). Tiny Rust JSON-RPC 2.0 daemon meant to be baked into container images for richer in-VM ops; talks to the host over a bind-mounted Unix socket at `/run/cersei-envd.sock`. Methods: `process.run`, `fs.{read,write,list,stat,mkdir,remove}`, `ping`, `info`. Reuses the JSON-RPC wire shape from `cersei-mcp/src/jsonrpc.rs`.
  - **Reference image.** `crates/cersei-vms/docker/Dockerfile` produces `cersei/sandbox-base:latest` — Alpine 3.20 + `bash` + `git` + `cersei-envd`, ~8 MB.
  - **Tests.** 9 passing — 4 LocalProcessRuntime (incl. snapshot round-trip preserving FS + KV state), 2 mailbox (cross-sandbox pub/sub + topic isolation), 3 KvStore (concurrent writes, CAS rejecting stale writers, journal-survives-reopen).
- **cersei-tools integration** behind a new `vms` feature.
  - **Transparent routing in `BashTool`.** When `ctx.extensions.get::<Arc<dyn cersei_vms::Sandbox>>()` is `Some`, the bash tool routes `RunRequest` through the sandbox instead of running on the host. Falls back to the existing local `pproc::exec()` path otherwise — fully backward-compatible for users not on the feature.
  - **New agent-facing tools** in `cersei_tools::vm_tools`: `SendVmMessage` / `RecvVmMessage` (cross-sandbox pub/sub with bounded wait timeout), `SharedStateGet` / `SharedStateSet` (KV with optional CAS), `SandboxSnapshot`. All gated by the existing `PermissionLevel` system (`Write` / `ReadOnly`).
- **`cersei` facade** — `cersei::vms` re-export behind a new default-on `vms` feature, so `use cersei::prelude::*` gains the sandbox surface without an extra import.

### Changed

- Workspace bumped to **0.1.9** (15-crate layout now includes `cersei-vms`).
- `Cargo.toml` workspace `members` and `[workspace.dependencies]` updated to register `cersei-vms` alongside the existing 14 crates.

### Deferred to 0.1.10

- **Agent-builder injection.** `AgentBuilder::with_sandbox(handle)` / `with_sandbox_allocator(alloc)` so each `Agent` (or each child of a delegate batch) lands inside its own sandbox automatically, without callers hand-stuffing `extensions`. Phase 1 leaves this as a call-site responsibility while we land the right type-shape (needs a generic extension-provider so `cersei-agent` doesn't permanently depend on `cersei-vms`).
- **`cersei-agent::delegate` per-task allocator.** Wire `Option<Arc<dyn SandboxAllocator>>` into `DelegateConfig` so each parallel worker gets a fresh sandbox + shared Volume/Mailbox.
- **abstract-cli surface.** `--sandbox <local|docker>` flag and `/vm` slash command (list / inspect / kill sandboxes attached to the current session).
- **Other backends.** `FirecrackerRuntime` (Linux-only microVMs), `E2bRuntime` (remote E2B API), `VercelSandboxRuntime`.
- **Phase 2 lifecycle.** `docker pause` / `unpause` on `DockerRuntime`, incremental snapshots, snapshot garbage collection, bi-directional stdin streaming on `Commands::stream`.

## [0.1.8] - 2026-04-25

### Added

- **Workstream A — Memory++ (partial).** Targeted retrieval improvements aimed at LongMemEval multi-session / temporal weak spots, on top of the 0.1.7 85.7 % hybrid baseline.
  - **Omega-style per-question-type RAG prompts** (`bench/long-mem/src/omega_prompts.rs`) — verbatim ports of Omega-memory's 5 answerer templates (vanilla, enhanced-confident, multi-session-aggregation, temporal, preference), routed by `QuestionType`. Stacks on top of Mastra's `OBSERVATION_CONTEXT_INSTRUCTIONS` in the system prompt, Omega template in the user turn.
  - **LLM-driven query expansion** (`bench/long-mem/src/query_expand.rs`) — Omega's verbatim `OMEGA_EXPANSION_SYSTEM` prompt producing `{lex, vec, hyde}` variants. HyDE gated by a `looks_vague` heuristic (≥ 80 chars or synthesis keyword). Tolerant JSON parser (fenced / chatty). Variants fused into RRF at `EXPANSION_WEIGHT = 0.8`.
  - **Abstention floors** (`VEC_MIN_SIM = 0.35`, `GRAPH_MIN_SCORE = 0.30`) in `HybridConfig::retrieve` — drops low-confidence hits before RRF.
  - **Semantic dedup at ingest** — normalized Jaccard ≥ 0.85 collapses near-duplicate facts to keep the retrieval set crisp.
  - **500Q `longmemeval_s` numbers on `gemini-2.5-flash` (2026-04-25):**
    - a-baseline-jsonl: **87.56 %** overall
    - b-embed-only: **86.56 %**
    - d-hybrid-embed-graph: **86.28 %** overall, **81.0 % multi-session (+2.5 pp vs 0.1.7 hybrid)**, 82.7 % temporal-reasoning, 90.3 % knowledge-update
    - A2 FTS5 channel and A3 three-way RRF deferred to 0.1.9 — that pair is what closes the remaining gap to the 90 % stretch target.
- **Workstream B — Skills primitive.** New leaf crate `cersei-skills` for loading [agentskills.io](https://agentskills.io/)-compatible SKILL.md files.
  - **`SkillRegistry`** — loads from `~/.cersei/skills/` (global) and `.cersei/skills/` (project); project-over-global merge; progressive-disclosure `list()` / `view()` / `by_tag()` surface.
  - **`Skill` parser** — YAML frontmatter (name ≤ 64 ASCII, description ≤ 1024, version, license, platforms, prerequisites) + markdown body. 100 KB size cap.
  - **Security scanner** — `SkillSecurityIssue::{PromptInjection, DestructiveCommand, CredentialExfil, InvisibleUnicode, SudoersOrSetuid}`; skills refuse to load until reviewed.
  - **`HookEvent::TurnsElapsed`** added to `cersei-hooks`; `cersei-agent::runner` emits it every `turns_elapsed_cadence` turns (default 10, 0 disables). Intended for background skill review nudges without blocking the agent loop. `AgentBuilder::turns_elapsed_cadence(n)`.
- **Workstream C — Delegation / parallel subagents.** Port of hermes-agent's `delegate_tool.py`.
  - **`cersei_agent::delegate`** module — `DelegateConfig`, `DelegateTask`, `DelegateResult`, `run_batch`. Bounded parallelism via `tokio::task::JoinSet` + semaphore (`DEFAULT_MAX_CONCURRENT = 3`). Depth cap `MAX_DEPTH = 2` (parent = 0, child = 1, no grandchildren). Default blocklist `{delegate, clarify, memory, memory_write, send_message, ask_user, AskUserQuestion}` stripped from child toolsets at depth ≥ 1. Child system prompt is a verbatim port of `_inspirations/hermes-agent/tools/delegate_tool.py::_build_child_system_prompt`.
  - **`cersei_agent::delegate_tool::DelegateTool`** — `impl Tool` wrapper. Accepts either a single `goal` or a `tasks` array. Fresh provider + toolset per child via `ProviderFactory` / `ToolsetFactory` `Arc<dyn Fn() -> Box<...>>` closures (the non-cloneable trait-object workaround).
  - **`AgentBuilder::provider_boxed(Box<dyn Provider>)`** — builder extension needed by the delegation pathway.
  - Best-effort semantics: one child failure doesn't abort the batch; the parent gets a per-task error summary.

### Changed

- `bench/long-mem/src/runner.rs` — answerer system prompt now splices `ANSWERER_BASE_SYSTEM` + Mastra `OBSERVATION_CONTEXT_INSTRUCTIONS`; user turn uses the Omega per-type template via `omega_prompts::prompt_for(question_type)`.
- `HybridConfig::retrieve` rewritten to perform multi-query RRF over `{primary, lex, vec, hyde}` with abstention floors; variants contribute at `EXPANSION_WEIGHT = 0.8`.

### Deferred to 0.1.9

- **Workstream A2/A3** — FTS5 channel on `cersei-memory` (behind new `fts` feature) and three-way RRF with per-type `RetrievalProfile` TOML. The biggest lever still unused.
- **Python RPC code-exec tool** — collapses multi-step tool chains into one LLM turn (terminal-bench unlock). Requires Rust-side UDS server + Python stub generator.
- **Workstream D** — `cersei-session-search` (FTS5), `HonchoMemoryProvider`, `cersei-cron`.

## [LongMemEval Benchmark] - 2026-04-24

### Added

- **LongMemEval head-to-head benchmark (`bench/long-mem/`).** Runs the 500-question [LongMemEval](https://arxiv.org/abs/2410.10813) (ICLR 2025) dataset — the same benchmark [Mastra](https://mastra.ai/research/observational-memory), [Zep](https://arxiv.org/abs/2501.13956), Supermemory, Hindsight, and EmergenceMem report on — against four Cersei memory configurations: full-context baseline, usearch-HNSW semantic (`EmbeddingMemory`), grafeo graph substring+rerank (`GraphMemory::recall_top_k`), and a hybrid config combining an Observer LLM pass + embedding + graph + RRF fusion. Judge rubric, observer rubric, and context-injection prompts are **verbatim ports** from Mastra's `@mastra/memory` so numbers land on the same public leaderboard. Docs: [Memory Benchmark](https://cersei.pacifio.dev/docs/bench-memory).
  - **Headline result on `longmemeval_s` (2026-04-24, all on `gemini-2.5-flash`):**
    - Baseline (full-context `JsonlMemory`): **84.6 %** overall, 86.7 % abstention, 422/500, 53.16 M input tokens.
    - Embed (`EmbeddingMemory`, usearch HNSW + `gemini-embedding-001`): **84.2 %** overall, 86.7 % abstention, 429/500, **2.68 M input tokens (20× fewer than baseline)**.
    - Graph substring (`GraphMemory`, grafeo): 6.6 % overall (honest floor — substring can't paraphrase-match), 100 % abstention.
    - **Hybrid (Observer + embed + graph + RRF):** **85.7 % overall, 93.3 % abstention, 432/500, 1.58 M input tokens (34× fewer than baseline).** Best config; wins outright on `knowledge-update` (94.4 %).
  - **Leaderboard position:** Cersei Hybrid 85.7 % beats Supermemory / `gemini-3-pro-preview` (85.2 %), Supermemory / `gpt-5` (84.6 %), Mastra OM / `gpt-4o` (84.23 %), Mastra RAG (80.05 %), Zep (71.2 %), and lands within 0.3 pp of EmergenceMem Internal on `gpt-4o` (86.0 %). Remaining gap to Mastra OM / `gemini-3-flash-preview` (89.2 %) and `gpt-5-mini` (94.87 %) is concentrated on answerer-model tier, not algorithm.
- **Mastra prompt port** (`bench/long-mem/src/mastra_prompts.rs`) — verbatim ports of `OBSERVER_EXTRACTION_INSTRUCTIONS`, `OBSERVER_OUTPUT_FORMAT`, `OBSERVER_GUIDELINES`, `OBSERVATION_CONTEXT_PROMPT`, `OBSERVATION_CONTEXT_INSTRUCTIONS` from `_inspirations/mastra/packages/memory/src/processors/observational-memory/`. Unit tests assert the required sections round-trip.
- **`cersei-memory::embedding_memory::EmbeddingMemory`** — thin adapter bridging `cersei-embeddings::EmbeddingStore` into the `Memory` trait. Behind the new optional `embed` feature so consumers opt in. Exposes `add`, `add_batch`, and standard `Memory::{store, search, delete}` with relevance-scored `MemoryEntry` return values.
- **`cersei-memory::graph::GraphMemory::recall_top_k(query, limit)`** — returns `Vec<(String, f32)>` where the score is the fraction of query words found in each memory. Additive; does not change the existing `recall` signature.
- **`cersei-embeddings::GeminiEmbeddings`** rewritten for `gemini-embedding-001` (3072-d native, Matryoshka `outputDimensionality` supported). Uses the `embedContent` endpoint with `futures::stream::buffered(20)` concurrency; retries 429 + 5xx + transport errors with exponential backoff (6 attempts, ~30 s window).

### Fixed

- **`cersei-embeddings::OpenAiEmbeddings::embed_batch`** no longer panics on multi-byte UTF-8 input. The truncation logic used raw byte slicing at index 2000, which panicked on any text containing non-ASCII characters (Spanish diacritics, emoji, smart quotes) when the character straddled the slice boundary. Now walks back to the nearest char boundary. Caught while running the LongMemEval bench against Spanish session content.

### Security

- **Gemini API keys moved from URL query string (`?key=…`) to the `x-goog-api-key` header** in both `cersei-provider::Gemini` and `cersei-embeddings::GeminiEmbeddings`. The URL now contains no secret, so `reqwest::Error` Display (which prints the URL) cannot leak the key. Prior code would expose the key in every transport error, and those errors rode into tracked `.log` files and per-question `rows-*.json` artifacts.
- **`redact_url_key` helper** in `cersei-embeddings/src/gemini.rs` — belt-and-braces scrubber applied to any error body that might still reference a URL carrying `key=…` (e.g. historic error strings, upstream error bodies).
- **`.gitignore` hardened** to cover `bench/**/*.log`, `bench/**/results*/`, `bench/**/runner-*.sh`, `bench/**/abstract-output.jsonl`, `bench/**/tb-results/`, and any `.env*` / `*.secret*` / `credentials*.json`.
- **`bench/term-bench/runner-google.sh`** no longer hardcodes a key; it fails fast unless `GOOGLE_API_KEY` is already in env.
- **38 previously-tracked bench artifacts** (logs, per-question rows, terminal-bench JSONL) removed from the git index. Numbers are reproducible by rerunning the bench.
- Pre-commit sanity check: `git ls-files | xargs grep -l -E "AIza[A-Za-z0-9_-]{35}|sk-[A-Za-z0-9_-]{30,}"` must return zero tracked files.

## [0.1.7] - 2026-04-20

### Added

- **New crate: `cersei-compression`.** Structural and command-aware compression for tool outputs, sitting between a tool's `execute()` result and the agent's existing `cap_tool_result()` truncation. Trims the tokens that typical tool output wastes on ANSI, comments, blank lines, and boilerplate. Three levels: `Off` (default — zero behaviour change), `Minimal` (ANSI + comment stripping, whitespace collapse), `Aggressive` (adds language-aware body stubbing for source files and declarative TOML rules for common CLIs: `git`, `cargo`, `npm`, `pnpm`, `pytest`, `docker`, plus a generic catch-all). Docs: [Overview](https://cersei.pacifio.dev/docs/compression-overview) · [Benchmarks](https://cersei.pacifio.dev/docs/compression-benchmarks).
  - **Credit:** this crate is a port of [**rtk** (Rust Token Killer)](https://github.com/rtk-ai/rtk) by **Patrick Szymkowiak**, MIT licensed. See `crates/cersei-compression/LICENSE` for full attribution and the per-module mapping table.
  - **`AgentBuilder::compression_level(level)`** — set at build time.
  - **`Agent::set_compression_level(level)` / `agent.compression_level()`** — change or inspect at runtime (shared-mutex, takes effect on the next tool call).
  - **Observability** — every call emits a structured `tracing::info!` event on target `cersei_compression` with `tool`, `level`, `strategy`, `detail` (matched rule or detected Language), `before_bytes`, `after_bytes`, `before_lines`, `after_lines`, and `savings_pct`. Subscribe with `RUST_LOG=cersei_compression=info`.
- **`abstract-cli` compression controls.**
  - **`--compress <off|minimal|aggressive>`** CLI flag, `ABSTRACT_COMPRESSION` env var, and `compression_level` in `~/.abstract/config.toml` / `.abstract/config.toml`.
  - **`/compression [on|off|minimal|aggressive]`** slash command flips the active agent's level mid-session. `/compression` with no argument reports the current level.
- **Live-provider savings benchmark** (`crates/cersei-agent/tests/e2e_openai_compression.rs`, `#[ignore]`). Same prompt, same fixture, same tool — only `CompressionLevel` changes between runs. Token counts are provider-reported, not our estimate.
  - **OpenAI `gpt-4o-mini`** — 11,576 → 8,202 input tokens (**−29.1%**, Δ 3,374 tokens; 15 → 13 tool calls).
  - **Google Gemini `gemini-2.5-flash`** — 4,490 → 1,700 input tokens (**−62.1%**, Δ 2,790 tokens; 1 → 1 tool call).
  - **Synthetic floors** (`crates/cersei-compression/tests/savings.rs`) — `git log` ≥ 30% at Minimal, `cargo test` ≥ 25% at Minimal, Off is byte-for-byte identity.
  - **Reproduce** — full commands and per-call log dumps on [Compression Benchmarks](https://cersei.pacifio.dev/docs/compression-benchmarks).

### Changed

- **Workspace version** — every crate (`cersei`, `cersei-agent`, `cersei-compression`, `cersei-embeddings`, `cersei-hooks`, `cersei-lsp`, `cersei-mcp`, `cersei-memory`, `cersei-provider`, `cersei-tools`, `cersei-tools-derive`, `cersei-types`, `abstract-cli`) bumped to **0.1.7** via `version.workspace = true`.
- **`cersei-agent::Agent` + `AgentBuilder`** gained a `compression_level` field wired through to the runner at `crates/cersei-agent/src/runner.rs:708`. Default is `CompressionLevel::Off` — existing users see no behavioural change without opting in.

## [0.1.6-patch.2] - 2026-04-18

### Added

- **New crate: `cersei-embeddings`.** Provider-agnostic text embeddings with a pluggable `usearch`-backed vector index, extracted from the inline embedding logic in `CodeSearch`. Ships with built-in `GeminiEmbeddings` (Google `text-embedding-004`, 768-d) and `OpenAiEmbeddings` (OpenAI `text-embedding-3-small`, 1536-d, base-URL overridable for Azure / Ollama).
  - **`EmbeddingProvider` trait** — `embed`, `embed_batch`, `dimensions`, `name`. Implement once, compose with everything below.
  - **`VectorIndex`** — thin `usearch` wrapper exposing `new`, `from_vectors`, `reserve`, `add`, `search`, `len`. Cosine / L2 / InnerProduct metrics with automatic similarity conversion.
  - **`EmbeddingStore<P>`** — provider + index bundled for the add-text / search-by-text flow. `new`, `add_batch`, `search`.
  - **`auto_from_model(&str)`** factory — picks OpenAI or Gemini based on an LLM model string and reads the appropriate env var.
  - **Leaf dependency** — the crate has zero dependencies on other `cersei-*` crates, usable standalone for RAG, semantic search, clustering, and custom tools.
  - **Docs** — [Overview](https://cersei.pacifio.dev/docs/embeddings-overview), [API Reference](https://cersei.pacifio.dev/docs/embeddings-api), [Cookbook](https://cersei.pacifio.dev/docs/embeddings-cookbook).
- **General-Agent Framework Benchmark.** First-party, end-to-end measured showdown against the Python stack — Agno 2.5.17, PydanticAI 1.22.0, LangGraph 1.1.8, CrewAI 1.14.2. Everything measured on Apple M1 Pro via the same harness suite, methodology mirroring Agno's own `cookbook/09_evals/performance/` (real agent constructors, no LLM invocation, no stub models). Three new chart components (`AgentInstantiationChart`, `PerAgentMemoryChart`, `MaxConcurrentChart`) now render on three pages.
  - **Headline numbers** — Cersei **704 B per agent** (8× smaller than Agno's 5.8 KiB, 44× smaller than LangGraph's 30 KiB). Cersei builds 500 agents concurrently in **4.4 ms / 8.5 MB** vs CrewAI's **50,697 ms / 1,739 MB** at the same N — **11,500× faster wall time, 204× less memory**. Cersei sweeps to 10,000 concurrent agents held live in 87 ms on 22 MB total RSS.
  - **Details & charts** — [cersei.pacifio.dev/docs/bench-vs-agents](https://cersei.pacifio.dev/docs/bench-vs-agents) (dedicated deep-dive page with all five axes, reproduction instructions, and caveats).
  - **General comparisons page** — [cersei.pacifio.dev/docs/comparisons](https://cersei.pacifio.dev/docs/comparisons#cersei-vs-general-agent-frameworks-agno-pydanticai-langgraph-crewai) (now includes side-by-side against all four Python frameworks + "when to choose which" guidance).
  - **Landing page performance section** — [cersei.pacifio.dev/docs](https://cersei.pacifio.dev/docs#vs-general-agent-frameworks) (the new "vs General Agent Frameworks" sub-section under Performance at a Glance).
  - **Harness source** — Rust: `crates/cersei-agent/benchmarks/general_agent_bench.rs` (opt-in via `bench-full` feature). `uv`-managed Python harnesses + `run.sh` at `bench/general-agents/` ([GitHub](https://github.com/pacifio/cersei/tree/main/bench/general-agents)). Reproduce end-to-end on your own machine with `./run.sh`.

### Changed

- **`cersei-tools::code_search`** refactored to delegate to `cersei-embeddings`. Inline `gemini_embeddings` / `openai_embeddings` / raw `usearch::Index` handling removed. `CodeSearchTool::with_embeddings` now takes `Arc<dyn EmbeddingProvider>` instead of `(provider_string, api_key_string)`.
- **`abstract-cli`** constructs its embedding provider via `cersei_embeddings::auto_from_model(&resolved_model)` — the model → provider detection and env-var lookup moved into the new crate. End-user behaviour of `--embedding-api` is unchanged.
- **`cersei-lsp`** `Cargo.toml` now uses `version.workspace = true` (was hardcoded to `0.1.6`) so workspace version bumps propagate.
- **Google provider default model** upgraded from `gemini-2.0-flash` to `gemini-3.1-pro-preview` (2M context). Affects `abstract --model gemini`, the `auto` fallback when `GOOGLE_API_KEY` is the only configured key, and `Gemini::new()` / `Gemini::builder()` when `.model(...)` is omitted.
- **`abstract login <provider>`** now accepts any provider registered in the `cersei-provider` registry (Google, Groq, DeepSeek, xAI, Mistral, Together, Fireworks, Perplexity, Cerebras, OpenRouter, Cohere, SambaNova, …), not just `anthropic` and `openai`. Saved keys are stored in a generic `provider_keys` map in `~/.abstract/credentials.json` and exported as the provider's first env var at startup so downstream registry lookups see them transparently. Local providers (Ollama) report "no login needed".

### Fixed

- **Auto-default no longer silently picks Ollama.** Two changes: (1) `cersei_provider::registry::available()` now TCP-probes local providers (those with no `env_keys`) via a 200ms check against their `api_base`, so `abstract login status` distinguishes `available (local)` from `not running`. (2) `from_model_string("auto")` now skips local providers entirely and only considers keyed providers (Anthropic, OpenAI, Google, Groq, …). Ollama and other local providers must be selected explicitly via `--model ollama/<model>`. When no API keys are set, the CLI errors out with a helpful message instead of silently defaulting to `llama3.1`.
- **`abstract login google`** (and every other provider known to the registry) is no longer rejected as "Unknown provider".

## [0.1.6-patch.1] - 2026-04-13

### Added

- **VibeProxy support.** Abstract CLI can now route requests through VibeProxy or any compatible local proxy, enabling use of existing AI subscriptions (Claude Pro/Max, ChatGPT Plus) instead of API keys.
  - **`--proxy` CLI flag** — force proxy usage even when API keys are set.
  - **`--proxy-url URL` flag** — specify a custom proxy URL (default: `http://localhost:8317/v1`).
  - **Auto-detection** — when no API keys are set and VibeProxy is running on `localhost:8317`, Abstract automatically routes through the proxy.
  - **`/proxy` slash command** — shows proxy status, URL, and authenticated accounts from `~/.cli-proxy-api/`.
  - **`[proxy]` config section** in `.abstract/config.toml` — `enabled`, `force`, `url` fields.
  - Header shows `model via proxy` when proxy is active.
- **Channel-based TUI permission system.** Permissions now flow through `tokio::sync::mpsc` + `oneshot` channels instead of reading from stdin directly. Fixes permission overlay freezing the TUI.
- **Virtualized message list.** `VirtualList` renders only visible items (O(viewport_height) per frame instead of O(total_lines)). Pre-built committed items are cached; only streaming content is rebuilt per frame. Buffer cleared before render to prevent stale content bleed-through.
- **Inline diff viewer.** File edit tools (Edit, Write, ApplyPatch) now show syntax-highlighted unified diffs inline in the conversation with `┌─ diff` / `│ +/-` / `└─` borders.
- **Edit tool captures diffs.** `file_edit.rs` reads file before and after edit, computes unified diff, includes it in the tool result.
- **Multi-line input.** Textarea with word wrapping, dynamic height (1-10 lines), and newline insertion via Option+Enter (macOS), Ctrl+J, or Shift+Enter (kitty protocol terminals).
- **Kitty keyboard protocol.** `PushKeyboardEnhancementFlags(DISAMBIGUATE_ESCAPE_CODES)` enabled at startup for terminals that support it. Shift+Enter works in iTerm2, Kitty, WezTerm, Alacritty.
- **4 new cookbook pages** — ML Coding Agent, Research Agent, General Agent (memory + skills + MCP), Graph Memory Deep Dive.
- **Comparisons page** — Cersei vs Claude Code SDK, Cersei vs Pydantic AI / LangChain with feature matrix.
- **Code & AST Intelligence docs** — full API breakdown for `cersei-lsp` and tree-sitter modules.

### Changed

- **Permission system rewritten** from stdin-based to channel-based for TUI mode. `TuiPermissionPolicy` sends requests via `mpsc`, TUI renders overlay, user decision sent back via `oneshot`. No more stdin race condition.
- Permission overlay enlarged to 75% x 55% with padding and `Wrap { trim: true }`.
- Recovery overlay also enlarged.
- Help overlay updated with all 17+ commands, keybindings, and permission modes.
- File tree in side panel now shows compact directory view with `▸ dir/ (count)` format instead of fully expanded tree.
- Git diff panel shows `git status --short` with human-readable labels (untracked/modified/added/deleted) instead of raw `??`/`M`/`A` codes.
- Side panel supports focus mode: Ctrl+B focuses panel, j/k scroll, Tab switches tabs, Esc returns to input. Yellow border when focused.
- Tool call output preview: file tools get 12 lines (up from 5), diff rendering for Edit/Write/ApplyPatch.

### Fixed

- **TUI permission freeze** — permission overlay no longer blocks input. Root cause: old `CliPermissionPolicy` called `crossterm::event::read()` and `enable_raw_mode()`/`disable_raw_mode()` while TUI was already in raw mode, causing stdin race condition and raw mode corruption.
- **Stale content in scrolled messages** — VirtualList now clears buffer area with `cell.reset()` before rendering visible items.
- **Resize crash in Ghostty** — kitty keyboard protocol only enabled if `supports_keyboard_enhancement()` returns true. Event drain capped at 50 per tick. Protocol re-pushed on resize.
- **Cost display $0** — CostUpdate and TurnComplete handlers now estimate cost from model pricing when provider reports $0.
- `/memory`, `/sessions`, `/model` no longer return "unknown command".

## [0.1.6] - 2026-04-12

### Added

#### New Crate: `cersei-lsp`
- **Language Server Protocol client** (`cersei-lsp`). On-demand LSP server management with JSON-RPC 2.0 over stdio. 5 operations: hover, definition, references, document symbols, diagnostics. Built-in configs for 13 language servers (rust-analyzer, pyright, typescript-language-server, gopls, clangd, ruby-lsp, phpactor, lua-language-server, bash-language-server, sourcekit-lsp, omnisharp, jdtls, zls). Auto-detection by file extension, lazy server startup.
- **LSP tool** in `cersei-tools` — agents can query language servers via `LSP` tool with action/file/line/column.

#### Tree-sitter Code Intelligence
- **Multi-language tree-sitter parsing** (Rust, TypeScript/JavaScript, Python, Go). Extracts imports and symbols (functions, structs, classes, interfaces, enums, modules, types).
- **Bash command safety analysis** (`bash_safety`) — tree-sitter AST-based risk classification (Safe/Moderate/High/Forbidden). Detects command substitution, process substitution, redirections, pipelines, privilege escalation. 8 tests.
- **Dependency-ranked project intelligence** (`code_intel`) — `scan_project()` discovers source files, parses with tree-sitter, scores by importance (entry points, import frequency, symbol count). Injected into system prompt as `project_intel`.

#### Production TUI
- **ratatui TUI** with alternate screen, `tokio::select!` event loop at 62 FPS.
- **Side panel** (Ctrl+B) — 38% right width, tabbed: Git Diff (status + colored diff) and File Tree (compact with file counts).
- **Permission modes** (Shift+Tab) — Auto, Plan, Editor, Bypass, Bypass+Alert.
- **Enterprise theme** — AMOLED black (`#000`), monochromatic, `#ffff00` accent, derived from Zed Enterprise theme. Default theme.
- **3 themes** — Enterprise, Light, Solarized.
- **Markdown rendering** with pulldown-cmark + syntect. Text wrapping at terminal width.
- **Graph visualization** (`/graph`) — memory node graph overlay.
- **16 slash commands** — `/help`, `/clear`, `/cost`, `/model`, `/memory`, `/sessions`, `/diff`, `/files`, `/panel`, `/graph`, `/undo`, `/rewind`, `/compact`, `/exit`.
- **Scrolling** — PageUp/Down, Home/End, Ctrl+Up/Down for side panel.
- **Bracketed paste** support. Native text selection (mouse capture disabled).

#### Agent Loop
- **Parallel tool execution** via `futures::future::join_all()`.
- **Automatic retry with exponential backoff** — 5 retries on 429/529, 1s→16s delays + jitter.
- **LLM-based context compaction** — provider-based summarization at 90% context, snip-compact fallback.
- **Todo nudge injection** — reminds model about incomplete tasks on turns > 2.
- **Depth nudge** — forces deeper exploration on early EndTurn after tool calls.
- **MaxTokens recovery** — 3 retries on output token limit.

#### File Operations
- **File snapshot/undo** (`file_snapshot`) — before/after content per tool call, `/undo` command.
- **ApplyPatch tool** — unified diff format patching.
- **Shell state persistence** — sentinel-based cwd capture across bash calls.

#### Provider Updates
- **GPT-5.x models** — `gpt-5.3-chat-latest` (default), `gpt-5.3-chat`, `gpt-5-chat`, `o3-pro`. 1M context windows.
- **`max_completion_tokens`** for GPT-5.x/o-series. **`stream_options: include_usage`** for token tracking.
- **Per-message cost estimation** — `estimate_cost()` with pricing for 15 models.

#### Memory & Config
- **AGENTS.md / CLAUDE.md hierarchy** — walks up directory tree collecting instruction files.
- **File watching** (`file_watcher`) — `notify` crate for project file change detection.
- **max_turns** increased from 20 to 50.

### Changed

- Default OpenAI model: `gpt-4o` → `gpt-5.3-chat-latest`.
- Default theme: generic dark → Enterprise (AMOLED black).
- `Agent::run_stream()` takes `Arc<Self>` instead of unsafe pointer cast.
- System prompt rewritten for deep exploration enforcement.
- Gemini tool results use actual function name instead of `tool_use_id`.
- Glob capped at 200 results. Per-result cap at 30KB.

### Fixed

- TUI streaming via `tokio::select!` (no longer blocks event loop).
- Mid-stream cancellation via `cancel_token`.
- OpenAI `max_tokens` → `max_completion_tokens` for GPT-5.x.
- Token stats and cost display.
- Git diff panel shows untracked files with readable labels.
- Markdown text wrapping. Paste handling. `cap_tool_result` bounds.

## [0.1.5] - 2026-04-07

### Added

- **`/sessions` and `/ls` slash commands** — list all sessions directly from the REPL. Addresses [#9](https://github.com/pacifio/cersei/issues/9).
- **Expanded `/help` output** — now shows CLI subcommands (`abstract sessions list`, `abstract --resume`, `abstract login status`) alongside REPL slash commands. Model aliases updated to include `4o`, `gemini`, `llama`.
- **Conditional system prompt components.** System prompt refactored from 6 static sections to 23 components (8 conditional). New sections: output efficiency, tool result summarization, sub-agent guidance (when Agent tool available), skills guidance (when Skill tool available), memory guidance (when memory configured), context management warning (when auto-compact on), git status snapshot (structured: branch, user, status lines, recent commits), MCP server instructions, language preference.
- **`GitSnapshot` struct** for structured git context in the system prompt (branch, user, status lines, recent commits).
- **New `SystemPromptOptions` fields** — `tools_available`, `has_memory`, `has_auto_compact`, `git_status`, `mcp_instructions`, `language`.
- 11 new system prompt tests covering all conditional components.

### Changed

- Abstract CLI `prompt.rs` now populates `tools_available`, `has_memory`, `has_auto_compact`, and `git_status` from config and the working directory. Git info is a structured `GitSnapshot` instead of a one-line string.
- System prompt includes output efficiency and tool result summarization by default (previously missing).

## [0.1.4] - 2026-04-06

### Added

- **Tool primitives** (`tool_primitives` module). 6 sub-modules exposing the low-level building blocks that all 34 built-in tools use: `diff` (unified diffs, line diffs, patch application via `similar`), `fs` (async read/write/edit/diff/patch/metadata), `process` (async command execution with shell selection), `http` (GET/POST/fetch_html), `search` (structured grep with ripgrep + glob), `git` (async status/diff/log/branch detection). 26 new tests.
- **Built-in tools reference** documentation page with complete input schemas for all 34 tools using TypeTable.
- **Tool primitives documentation** — overview page, full API reference, and cookbook with DiffTool, deploy verifier, research agent, and git-aware code reviewer examples.
- **Providers documentation** page covering all 13 providers with env vars, models, context windows, and usage examples.

### Changed

- `file_read.rs`, `file_write.rs`, `file_edit.rs` refactored to delegate to `tool_primitives::fs`.
- `bash.rs` refactored to delegate to `tool_primitives::process::exec` (ShellState preserved).
- `web_fetch.rs` refactored to delegate to `tool_primitives::http::fetch_html`.
- `grep_tool.rs` refactored to delegate to `tool_primitives::search::grep` (structured `SearchMatch` results).
- `glob_tool.rs` refactored to delegate to `tool_primitives::search::glob`.

## [0.1.3] - 2026-04-05

### Added

- **Session auto-fork.** When a session file exceeds 50MB, writes automatically fork to a new part file (`session_part2.jsonl`, `_part3.jsonl`, etc.). Loading stitches all parts together transparently. Tombstones apply across parts. Total session limit across all parts is 200MB.
- **Multi-part session helpers** — `all_part_paths()` and `total_session_size()` for inspecting session files programmatically.
- **5 new session tests** — multi-part load, tombstones across parts, auto-fork path resolution, total size calculation.
- **Sessions & Tasks documentation** — two new doc pages (`sessions.mdx`, `background-tasks.mdx`) covering the full session lifecycle, auto-compact, memory extraction, auto-dream consolidation, task orchestration with programmatic code samples, cron scheduling, and git worktree isolation.

### Changed

- `load_transcript()` now loads from all part files and applies tombstones across the combined set.
- `abstract sessions rm` now removes all part files, not just the base.

### Fixed

- Sessions that exceeded 50MB became unloadable. Now they auto-fork before hitting the limit.

## [0.1.2] - 2026-04-04

### Added

- **Multi-provider model router** (`registry.rs`, `router.rs`). 13 providers supported out of the box via `provider/model` string format. Most providers reuse the existing OpenAI-compatible client with a different base URL — zero new SSE parsing per provider.
- **Provider registry** with API base URLs, env var names, default models, context windows, and capabilities for: Anthropic, OpenAI, Google (Gemini), Mistral, Groq, DeepSeek, xAI (Grok), Together, Fireworks, Perplexity, Cerebras, Ollama, OpenRouter.
- **`from_model_string()`** top-level function on `cersei-provider` — parses `"groq/llama-3.1-70b-versatile"` into a configured `Box<dyn Provider>`.
- **Auto-detection** from bare model names — `"gpt-4o"` routes to OpenAI, `"claude-sonnet-4-6"` to Anthropic, `"gemini-2.0-flash"` to Google.
- **Model aliases** in Abstract CLI — `--model llama`, `--model deepseek`, `--model grok`, `--model gemini`, `--model mistral`, `--model 4o`.
- **`abstract login status`** now shows all 13 providers with auth detection from environment variables.
- **Providers documentation page** (`docs/content/docs/providers.mdx`) covering every provider with env vars, models, context windows, and usage examples.

- **Provider continuity** — interactive model switching on provider errors. On rate limit or outage, the REPL shows options to retry, switch to a fallback model, wait, or skip. Conversation history transfers across provider switches via `AgentBuilder::with_messages()`.
- **`--fallback` CLI flag** — configure fallback models for provider switching (e.g., `--fallback groq/llama-3.1-70b-versatile,google/gemini-2.0-flash`).
- **`fallback_models`** config field and `ABSTRACT_FALLBACK_MODELS` environment variable.
- **OpenAI tool calling** — full streaming tool call support for OpenAI-compatible providers. Accumulates `delta.tool_calls` chunks across SSE events, emits proper `ContentBlockStart`/`InputJsonDelta`/`ContentBlockStop` events. Previously tool calls were silently dropped.
- **OpenAI message serialization** — assistant messages with tool calls now serialize as `tool_calls` array. Tool results serialize as `role: "tool"` with `tool_call_id`. Previously all messages were flattened to text, breaking the tool call loop.
- **`AgentBuilder::with_messages()`** — pre-populate conversation history when building an agent. Used for provider switching mid-session.
- **Session load guard** — runner skips loading session history from memory when messages are pre-populated via `with_messages()`, preventing duplicates.

### Changed

- Abstract CLI provider resolution replaced with the model router. The `provider` config field is now optional — the model string is the source of truth.
- Default model format changed to `provider/model` (e.g., `anthropic/claude-sonnet-4-6`).
- REPL now owns the Agent (instead of borrowing), enabling mid-session provider swaps.
- `build_agent()` extracted as a standalone reusable function in `app.rs`, called on both startup and provider switch.

### Fixed

- Grafeo graph database dependency now uses crates.io (`grafeo = "0.5"`) instead of local filesystem paths. `cargo install` no longer fails on machines without the author's directory layout.
- OpenAI tool calling loop — the agent no longer repeats the same tool call indefinitely. Tool results are now correctly serialized in OpenAI's expected format (`role: "tool"`, `tool_call_id`), allowing the model to see results and proceed.

### Removed

- `resolve_provider()` and `resolve_provider_name()` from Abstract CLI — replaced by `cersei_provider::from_model_string()`.
- Hardcoded local filesystem paths from all `Cargo.toml` files.

## [0.1.1] - 2026-04-03

### Added

- **Schema versioning and migration engine** (`graph_migrate.rs`). Graph databases now store a `(:SchemaVersion)` node. On open, the code checks the version and runs sequential migrations automatically. Each migration is idempotent.
- **Confidence decay**. Memory nodes track `last_validated_at` and `decay_rate`. `effective_confidence()` computes time-decayed confidence at read time — old memories lose weight without manual cleanup.
- **Embedding readiness**. Memory nodes include `embedding_model_version` (empty by default) preparing for future vector-based semantic recall.
- **`revalidate_memory(id)`** resets the decay clock on a memory node.
- **`schema_version()`** on `GraphMemory` for inspecting the current graph version.
- **Centralized GQL queries**. All 15+ scattered query strings in `graph.rs` extracted into a `mod gql` block.

### Changed

- `GraphMemory::open()` and `open_in_memory()` now auto-detect schema version and migrate on startup. No API change — MemoryManager callers are unaffected.
- `store_memory()` writes v2 fields (`last_validated_at`, `decay_rate`, `embedding_model_version`) on new nodes. Public signature unchanged.
- Default provider in Abstract CLI changed from `anthropic` to `auto` (detects from environment variables).
- `README.md` updated with Abstract CLI section, three-way benchmark table, and docs link.

### Fixed

- Empty `ANTHROPIC_API_KEY` environment variable no longer treated as valid auth.
- `run_tool_bench_claude.sh` (renamed from `run_tool_bench.sh`) — `grep` recall measurement no longer fails silently on no match.

## [0.1.0] - 2026-04-02

Initial release.

### Core SDK

- **cersei-types**: Provider-agnostic types — `Message`, `ContentBlock`, `Usage`, `StopReason`, `StreamEvent`, `CerseiError`.
- **cersei-provider**: `Provider` trait with Anthropic and OpenAI implementations. SSE streaming, token counting, prompt caching, extended thinking, OAuth support.
- **cersei-tools**: `Tool` trait, 34 built-in tools across 7 categories (filesystem, shell, web, planning, scheduling, orchestration, other). `#[derive(Tool)]` proc macro. Permission system with 6 levels. Bash command safety classifier. Skill discovery (Claude Code + OpenCode formats). Shell state persistence across invocations.
- **cersei-agent**: `Agent` builder with 20+ configuration options. Agentic loop with tool dispatch and multi-turn conversations. 26-variant event system (`AgentEvent`). `AgentStream` with bidirectional control. Auto-compact at configurable context threshold. Effort levels (Low/Medium/High/Max). Sub-agent orchestration. Coordinator mode. Auto-dream background consolidation. Session memory extraction.
- **cersei-memory**: `Memory` trait with `JsonlMemory` and `InMemory` backends. `MemoryManager` composing 3 tiers: Grafeo graph DB, flat files (memdir), CLAUDE.md hierarchy. Session storage via append-only JSONL with tombstone soft-delete. Auto-dream 3-gate consolidation system.
- **cersei-hooks**: `Hook` trait for lifecycle middleware. Pre/post tool use, model turn events. `ShellHook` for external command integration.
- **cersei-mcp**: MCP client over JSON-RPC 2.0 stdio transport. Tool discovery, resource enumeration, environment variable expansion.
- **cersei**: Facade crate re-exporting all sub-crates via `prelude::*`.

### Graph Memory

- Grafeo embedded graph database with 3 node types (`Memory`, `Session`, `Topic`) and 2 edge types (`RELATES_TO`, `TAGGED`).
- Graph recall in 98 microseconds (indexed lookup vs file-by-file text scan).
- Graph ON adds zero overhead to scan and context building, 92.5% faster recall.

### Benchmarks

- Tool dispatch: Edit 0.02ms, Read 0.09ms, Grep 6ms, Bash 17ms.
- Memory scan: 1.2ms for 100 files.
- Session I/O: 27us write, 268us load (100 entries).
- Context build: 45us (CLAUDE.md + MEMORY.md).

### Examples

- `simple_agent`, `custom_tools`, `streaming_events`, `multi_listener`, `resumable_session`, `custom_provider`, `hooks_middleware`, `benchmark_io`, `usage_report`, `coding_agent`, `oauth_login`.
- 5 stress test suites: core infrastructure, tools, orchestration, skills, memory. 160 unit tests, 262 stress checks.

### Documentation

- 10 markdown guides covering getting started, providers, tools, agent lifecycle, events, memory, hooks, permissions, architecture, benchmarks.
