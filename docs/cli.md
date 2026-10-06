# The `bricks` command

`bricks` is a thin layer over the engine (`cersei-agent`): it parses
arguments, loads the configuration, opens a session and presents its
events. The engine runs everything else — the agentic loop, providers,
tools, approvals, compaction, long-term memory recall and maintenance.

```text
bricks-cli (binary `bricks`)
  ├── arguments, configuration (existing loaders)
  ├── headless mode: human output or versioned JSONL
  └── bricks-tui: the interactive interface
            │  commands ↓   ↑ events      (cersei_agent::control)
       engine: Controller → Agent
            │
  providers · context · tools · approvals · memory · sessions
```

The engine never depends on `bricks-cli`, ratatui or crossterm. Both
frontends use the same contract (`cersei_agent::control`): typed
`Command`s in, versioned `Envelope`s out.

## Installation

```bash
cargo install --path crates/bricks-cli
```

This builds the `bricks` binary. The long-term memory uses USearch, a C++
library: a C++17 toolchain is needed to build. During development, run it
with `cargo run -p bricks-cli -- <arguments>`.

## Configuration

Two files are read, both through the existing loaders. Nothing is built in.

* **Models**: `~/.bricks/providers.toml`, or `--providers <file>`, or
  `$BRICKS_HOME/providers.toml` when `BRICKS_HOME` is set (see
  `docs/providers.md`). A model is named `provider_id/model_id`; its
  reasoning profiles, capabilities and prices come from this file.
* **Project**: `./bricks.toml` in the working directory (`--cd <dir>`
  changes it). Sections: `[agent]`, `[permissions]`, `[memory]`,
  `[context]`, `[compression]`, `[web]` (see `docs/bricks.example.toml`).
  An invalid section falls back to its defaults with a diagnostic, shown
  as a warning when the session opens.

```toml
[agent]
model = "my-provider/my-model"   # default model (otherwise --model is required)
reasoning = "deep"               # one of that model's reasoning profiles
max_turns = 50

[permissions]                    # the approval policy (below)
write = "ask"
execute = "ask"

[permissions.tools]
WebFetch = "allow"
```

Sessions are stored in `~/.bricks/sessions` (`$BRICKS_HOME/sessions`).

No model is ever chosen silently: without `--model` or `[agent] model`,
`bricks` stops and lists the configured models.

## Commands

| command | what it does |
|---|---|
| `bricks`, `bricks tui` | open the interactive interface (new session) |
| `bricks run "…"` | run one prompt without the interface |
| `bricks run --model <p/m> --reasoning <profile> "…"` | with another model / profile |
| `bricks run --json --non-interactive "…"` | JSONL on stdout, no human interaction |
| `bricks run --session <id> "…"` | continue a stored session headless |
| `bricks sessions [--json]` | list stored sessions, most recent first |
| `bricks resume` | open the interface on the session picker |
| `bricks resume <id>` | open the interface on that session |

Global options: `--cd <dir>`, `--providers <file>`, `--model <p/m>`,
`--reasoning <profile>`.

## Headless mode (`bricks run`)

It works without a terminal: no terminal mode is changed.

**Prompt and stdin.** The rules are fixed:

| invocation | prompt |
|---|---|
| `bricks run "text"` | `text`; stdin is not read, even when piped |
| `bricks run -` | stdin |
| `bricks run` (stdin not a terminal) | stdin |
| `bricks run` (stdin is a terminal) | error, exit code 2 (never waits) |
| `bricks run "text" --stdin` | `text`, then stdin as a second block (`<stdin>…</stdin>`) |

`--file <path>` and `--image <path>` attach files and images. Their
content is captured when the run starts.

**Output.** Without `--json`, the answer is streamed to stdout. Tool
calls, notices and errors go to stderr. With `--json`, stdout carries
only JSONL envelopes (schema below). Diagnostics and logs go to stderr.

**Approvals.** They are asked on the controlling terminal (`/dev/tty`)
when there is one, with the diff of a file change. With
`--non-interactive`, or without a terminal, nobody is waited for. A step
the policy says to ask about then stops the run, with `run_finished`
`failure = "approval_required"` and exit code 3. `--non-interactive`
grants nothing: steps the policy allows still run, the others do not.

**Ctrl+C** cancels the run (or the memory maintenance after it). A second
Ctrl+C exits at once. The session is stored. Tool calls interrupted by the
cancellation get a result saying so, so the session can be continued.
Effects they already had remain: nothing is "undone".

**Memory maintenance.** After the answer, the long-term memory (when
enabled) processes the exchange. `bricks run` waits for it unless
`--no-wait-memory` is given. Its work is durable and resumes at the next
run.

### Exit codes

| code | meaning |
|---|---|
| 0 | the run succeeded |
| 1 | the run failed (provider, engine, tool infrastructure), or stdout closed |
| 2 | invalid arguments or configuration, unknown model or session, refused attachment: nothing ran |
| 3 | a step needed an approval and nobody could give it |
| 4 | the answer was delivered, but the long-term memory maintenance failed |
| 130 | cancelled (Ctrl+C) |

## The JSONL schema (version 1)

Every line is one envelope:

```json
{"schema":1,"session_id":"20261006-141502-a1b2c3","run_id":"run_5f…","seq":7,"at":1791300902123,"type":"tool_started","tool_call_id":"call_1","name":"Glob","input":{"pattern":"*.md"}}
```

| field | |
|---|---|
| `schema` | `1`; incremented on any change a consumer could notice |
| `session_id` | the session |
| `run_id` | the run (absent for session-level events) |
| `seq` | 1, 2, 3, … contiguous: a gap never happens silently |
| `at` | milliseconds since the Unix epoch |
| `type` | the event (below); its fields follow, flat |

Events (`type`):

| type | fields | |
|---|---|---|
| `session_opened` | `working_dir, model, reasoning?, resumed, message_count, warnings` | first event |
| `run_started` | `prompt, attachments[{kind,path,bytes,sha256?,note?}], model, reasoning?` | |
| `text_delta` | `text` | the answer, streamed |
| `thinking_delta` | `text` | only reasoning the provider exposed; never reconstructed |
| `tool_started` | `tool_call_id, name, input` | concurrent calls have distinct ids |
| `tool_progress` | `tool_call_id?, name, message` | long calls (shell) |
| `tool_finished` | `tool_call_id, name, is_error, duration_ms, output` | |
| `approval_requested` | `approval{approval_id, tool_call_id, tool, level, description, input, preview?}` | `preview.files[{path, kind, before_sha256, diff, added, removed}]` |
| `approval_resolved` | `approval_id, tool_call_id, decision, by` | `decision`: `allow`, `allow_for_session`, `deny`; `by`: `user`, `session`, `non_interactive`, `cancelled` |
| `edit_applied` | `tool_call_id, tool, files[{path, kind, added, removed}]` | an approved, previewed change was written |
| `memory_recalled` | `items, tokens, omitted, budget` | what went into the system prompt |
| `context` | `status{context_used{tokens, provenance, …}, context_window, input_limit, totals, …}` | `provenance`: `measured`, `counted`, `mixed`, `estimated` |
| `usage` | `turn{…}, total{…}` | observed token usage; `cost_usd` absent = unknown, not zero |
| `compaction` | `reason, outcome, compacted` | |
| `model_changed` | `model, reasoning?, applies` | `next_turn` during a run, `next_run` otherwise |
| `context_cleared` | `messages_removed` | |
| `session_saved` | | |
| `notice` | `message` | retries, configuration diagnostics, engine nudges |
| `run_finished` | `outcome, failure?, error?, text, turns, approvals_unsatisfied?` | **exactly one per run**; `outcome`: `succeeded`, `failed`, `cancelled`; `failure`: `approval_required`, `error` |
| `memory_maintenance_started` | | after `run_finished` |
| `memory_maintenance_finished` | `outcome, report?, error?` | `outcome`: `completed`, `cancelled`, `failed` |
| `command_rejected` | `command, reason` | nothing changed |

A short run:

```text
{"schema":1,"session_id":"…","seq":1,"at":…,"type":"session_opened","working_dir":"/p","model":"demo/scripted","resumed":false,"message_count":0,"warnings":[]}
{"schema":1,"session_id":"…","run_id":"run_…","seq":2,"at":…,"type":"run_started","prompt":"Find the README","attachments":[],"model":"demo/scripted"}
{"schema":1,…,"seq":3,"type":"tool_started","tool_call_id":"call_0","name":"Glob","input":{"pattern":"*.md"}}
{"schema":1,…,"seq":4,"type":"tool_finished","tool_call_id":"call_0","name":"Glob","is_error":false,"duration_ms":3,"output":"README.md"}
{"schema":1,…,"seq":9,"type":"text_delta","text":"I listed the Markdown files. …"}
{"schema":1,…,"seq":14,"type":"run_finished","outcome":"succeeded","text":"…","turns":3}
```

No API key, authentication header or secret appears in any event. The
views the frontends read do not carry them.

### Commands (for frontends)

```json
{"type":"submit","prompt":{"blocks":[{"kind":"text","text":"…"},{"kind":"file","path":"src/a b.rs"}]}}
{"type":"cancel"}
{"type":"set_model","model":"p/m","reasoning":"deep"}
{"type":"compact"}
{"type":"clear_context"}
{"type":"resume","session_id":"…"}
{"type":"approve","approval_id":"ap_…","decision":"allow_for_session"}
```

Rules during a run:

* `cancel` is always accepted and reaches the run at once.
* `set_model` is accepted and applies from the next turn (`model_changed`
  says so). Profiles are the configuration's; there is no fixed list.
* `submit`, `resume`, `compact` and `clear_context` are refused
  (`command_rejected`). A second prompt never starts silently in
  parallel.

Presentation gestures (opening a window, expanding a block) are not
commands: they stay in the frontend.

### Delivery

Events reach each consumer through a bounded queue:

* text and reasoning deltas are merged when the consumer is slow. The
  text stays whole; only the number of events drops.
* any other event waits for room, so the run slows to the consumer's
  pace. Nothing is dropped.
* a cancellation is never held back by a slow consumer. Once the run is
  cancelled, its last events are queued beyond the bound.
* when the consumer goes away (closed stdout, closed interface), the run
  is cancelled (headless) or the session closes.

The durable session store is the authority on the conversation. The
interface keeps no conversation store of its own. On resume it reads the
stored history.

## Approvals

The policy belongs to the engine and is the same for the interface and
the headless mode. It is configured in `[permissions]`:

* by tool name first (`[permissions.tools]`), then by permission level
  (`read_only`, `write`, `execute`, `dangerous`, `none`), then `default`;
* actions: `allow`, `ask`, `deny`;
* defaults: reading is allowed, writing and executing are asked;
  `forbidden` tools are always refused.

When a call must be asked about:

* **File changes** (`Write`, `Edit`, `MultiEdit`, `ApplyPatch`). The tool
  computes the change without writing: files, kind, diff, and the SHA-256
  of each file at that time. Nothing is written before the decision.
* **A file changed meanwhile.** If a file changed while the decision was
  pending, the approval is not applied to the new state. The change is
  recomputed and asked again; after three changes in a row it is refused.
  The tiny window between that check and the write is not locked.
* **Rejection.** A rejected change writes nothing and leaves your files as
  they are.
* **Shell commands and MCP calls** have no file preview: their effects
  cannot be shown as a diff, and none is pretended. The approval shows the
  command and its context.
* **Allow for the session** applies to later calls of the same tool in the
  session.

## The interactive interface

* **Layout.** The interface is conversational: the transcript, a composer
  at the bottom and a one-line status bar. It uses an inline viewport. The
  live part (the run in progress, the composer, the status) stays at the
  bottom; finished parts are written once into the terminal's own
  scrollback, so the transcript stays in your terminal after you quit.
* **Windows.** Inspectors, pickers and the diff open on the alternate
  screen, and closing them restores the transcript untouched.
* **Long outputs** are never redrawn on every frame. In the transcript, a
  tool call shows one line (or its first error line). Runs of more than six
  calls are grouped. The details of the last calls (outputs up to 256 KB
  each; originals are stored with the session) open with Ctrl+O.
* **Reasoning** appears as a collapsed block only when the provider exposed
  some. Ctrl+T shows it live; Ctrl+O shows it after the fact.
* **Status bar.** It shows the model, the profile, the context used and the
  prompt budget, with the provenance (`est.`, `~`, `counted`, or nothing for
  measured). The cost is "unknown (no price)" when the model has no price.
  The full context and cost inspectors separate the current context from
  the cumulative consumption.

### Keys

| key | |
|---|---|
| Enter | send |
| Shift+Enter, Alt+Enter, Ctrl+J | newline (Shift+Enter needs a terminal that reports it, e.g. kitty, WezTerm, foot, recent iTerm2) |
| `\` then Enter | newline, on any terminal |
| paste | inserted as text (bracketed paste): never sent |
| `@` | file and folder list (fuzzy, `.gitignore` respected); Tab/Enter attach, Ctrl+R refresh, Esc close; `@"name with spaces` |
| `/` | command list |
| ↑ / ↓ | prompt history (on the first / last line); otherwise move the cursor |
| Ctrl+←/→, Alt+←/→ | by word; Shift selects |
| Ctrl+W, Ctrl+U | delete a word, clear the input |
| Backspace on an empty input | remove the last attachment |
| Ctrl+C | cancel the run or the memory maintenance; idle: clear the input, twice in a row: quit |
| Ctrl+D | quit (on an empty input) |
| y / a / n / d | with a pending approval: allow, allow for the session, reject, open the diff. Tab moves between the approval and the input |
| Ctrl+T, Ctrl+O | live reasoning; details of the last tools and reasoning |

### Commands

`/model [p/m [profile]]` · `/memory` · `/context` · `/cost` · `/session` ·
`/resume [id]` · `/compact` · `/clear` · `/diff` · `/tools` · `/mcp` ·
`/config` · `/file <path>` · `/folder <path>` · `/image <path>` · `/help` ·
`/quit`.

* **Registry.** The registry is declarative (name, aliases, arguments,
  description, action). Engine actions are contract commands; the others
  only open windows.
* **`/diff`** shows three different things apart: the change waiting for a
  decision, the changes applied by the agent in this session, and the
  working tree's own `git diff`, which includes your changes.
* **`/config`** never shows keys or authentication headers.

### Attachments

* **What.** Files, folders and images come from `@` or `/file`,
  `/folder`, `/image`.
* **Capture.** A file's content is captured when the prompt is sent: its
  path, size and SHA-256 go into the message, and the session keeps
  exactly what was sent. A later edit of the file does not change the
  history.
* **Folders** become a bounded listing (300 entries, depth 4,
  `.gitignore` respected), never their recursive contents.
* **Images** are sent as image blocks. The provider refuses them before
  sending when the model does not declare image input.
* **Paths** are resolved against the working directory when the prompt is
  sent. A missing one refuses the submission, so nothing is sent.

## Sessions and resume

`bricks resume` (picker) or `bricks resume <id>`, `/resume`, and
`bricks run --session <id>` restore:

* the conversation (the active history and the raw history) and the
  compaction snapshots;
* the working directory;
* the model and reasoning profile;
* the long-term memory space, recorded with the session (a different
  current space is reported as a warning).

A working directory that disappeared, or a model no longer in
`providers.toml`, is reported and nothing is resumed. No replacement is
chosen silently; `--model` picks one explicitly.

The persistent shell has its own life cycle. A resumed session starts a
new shell: processes, background tasks and shell variables of the old one
are not restored.

Deleting a session (`Memory::delete`) removes its history, raw history,
snapshots, saved tool outputs and `session.json` (its settings).

## Long-term memory

With `[memory] enabled = true` (see `docs/memory.md`), each run's prompt
recalls stored facts (`memory_recalled`). After the answer, the exchange
is processed as a separate, cancellable phase
(`memory_maintenance_started` / `_finished`). That phase is durable and
resumable: an interrupted pass leaves its work pending, and a successful
extraction is never repeated. A maintenance failure is reported on its
own; it never turns into a "memory success", and the delivered answer is
unaffected.

## A demonstration without any key

`scripts/demo_model.py` is a local scripted model server (Python standard
library). It speaks `chat_completions` on 127.0.0.1:8765, with
`scripts/demo_providers.toml`.

```bash
python3 scripts/demo_model.py &
export BRICKS_HOME="$PWD/.demo-home"
printf '[agent]\nmodel = "demo/scripted"\n' > bricks.toml   # in a scratch directory
bricks --providers scripts/demo_providers.toml run --json --non-interactive "Find the README"
bricks --providers scripts/demo_providers.toml run --non-interactive "please write a file"; echo "exit $?"   # 3: approval needed
bricks --providers scripts/demo_providers.toml sessions
bricks --providers scripts/demo_providers.toml            # the interface
```

## Tested platforms

* **macOS (arm64).** This is the only platform these tests and the PTY
  checks ran on: a Ctrl+C in headless mode, and the interface's modes,
  paste, resize, Ctrl+C and quit. The `/dev/tty` approvals and the PTY
  tests are Unix-only.
* **Linux.** Not run.
* **Windows.** Not run: the headless mode builds without `/dev/tty`
  approvals (no prompt is offered), and the interface was not exercised.

## Known limits

* The engine keeps its built-in nudges, notices included. After an early
  answer that used tools, it asks once for a deeper look. A first answer
  without tools is retried once with a forced tool call. This is existing
  engine behaviour, not a CLI choice.
* Tool progress events carry the tool's name, not its call id (the
  engine's progress hook does not know the call).
* Sub-agent events are not forwarded. A sub-agent is identified by the
  `tool_call_id` of the call that started it.
* No MCP server is configured from `bricks.toml` yet: `/mcp` says so.
* No syntax highlighting, clipboard access or in-terminal image preview.
  Images are attached by path.
* A panic restores the terminal through a panic hook. This is not covered
  by an automated test.
