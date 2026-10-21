# Bricks Desktop tests

## Running

```bash
cd apps/bricks-desktop
npm run check        # svelte-check + TypeScript
npm run test:unit    # Vitest (tests/unit)
npm run test:types   # type-check of the tests
cargo test -p bricks-desktop                       # Rust services, IPC, engine bridge
cargo test -p bricks-agent --test approval_override  # engine approval policy
```

Rust tests use temporary folders (`tempfile`, a temporary Bricks home); none reads or writes `~/.bricks`.

## Layout

- `tests/unit/` — Vitest, pure logic and state: conversation reducer and command labels, drafts and
  session creation, `/` palette and dispatch, `@` mentions, models and reasoning profiles, settings,
  theme and colours, context line, tool lines, run summary, terminals (with fake xterm/PTY ports).
- `src-tauri/tests/` — Rust integration tests: slash command mapping, provider and profile editing,
  preferences, skills prompt, run labels.
- `tests/e2e/` — WebdriverIO + `tauri-driver` specs. Tauri WebDriver runs on Linux and Windows only
  (no WKWebView driver on macOS); the WebdriverIO dependencies are not installed by default.

## Manual checks on macOS

Run the app (`npx tauri build --bundles app`, then open `target/release/bundle/macos/Bricks.app`) and check:

- Start-up with a configured provider; settings open from the bottom-left button.
- Projects: add a folder, switch projects; drafts stay per project with their model and effort.
- « Nouvelle conversation » repeatedly: no new stored conversation until the first message is sent.
- Streaming reply, tool lines (name, ms, state), approval with preview, allow / deny, cancel.
- `/` palette by keyboard; `/skill <name> <request>` shows its label, kept after a restart.
- `@` mentions, attachments (pick, paste, drop), input history (↑/↓).
- Context line and cumulative usage; sub-agents and isolated changes; changes panel (conversation,
  history, Git) and diffs.
- Terminal tabs: one visible, distinct output, resize, close.
- Appearance: light / dark / system, custom colours kept after a restart; Geist fonts.
- Window sizes (small, medium, large), zoom, keyboard focus.

Note: each new build changes the app's signature, so macOS asks again for access to protected folders
(e.g. the Desktop) used by a project; Bricks waits on that prompt at start-up.

### Concurrent execution and chat details

Automated: `crates/bricks-tools/tests/{shell_session,shell_recipe,edit_recipe}.rs`,
`crates/bricks-agent/tests/{edit_runtime,edit_history,bash_run_lifetime}.rs`,
`src-tauri/src/commands/agent_file.rs` (tests), `tests/unit/{context-gauge,context-card-structure,copy-inspector-terminal-structure,edit-formats}.test.ts`.

In the app, with a test project (never a real conversation):

- `Edit` on a file never read in the session; one `Edit` with `edits` (several replacements, one write).
- Four Bash calls of one turn run together (same start second); a long command does not block a quick one.
- Composer ring left of the model select: neutral without a conversation; hover or focus shows
  « Contexte », the compact value with its percentage, a thin bar and the cache rate (`—` without counts);
  light and dark.
- Context card: one line without `≈`; « Consommation session »: Entrée, Sortie, Cache, Requêtes, Coût.
- One « Copier » per finished response (after its last item, before the summary): it copies every answer
  segment of the run, in order, Markdown source only (✓ for a moment); still there after a restart.
- Sub-agent details: no « Identifiant » / « Parent »; task and result rendered as Markdown; « Fichiers
  modifiés » shows file names, a click reveals the file in the Finder (nothing is opened).
- Terminal: the cross after `+` hides the pane; reopening shows the same tab, output and height.

### Responses, summaries and reader

Automated: `src-tauri/src/services/run_records.rs`, `src-tauri/src/commands/reader_file.rs` (tests),
`tests/unit/{responses,run-records,run-worked,conversation-changes,reader-tabs,reader-operations,render-links-runend}.test.ts`.

In the app, with a test project:

- Summary: « Terminé · A travaillé MM:SS »; « N fichiers modifiés +A −D » and « N commandes exécutées »
  closed by default; a refused call is not counted; a sub-agent's commands are listed; white background in
  the light theme.
- Edit rows: name and counts, full path in the tooltip; a click opens the reader.
- Reader (« Lecteur », top right): one tab per file with a close cross; Source (highlighted, numbered,
  Précédentes/Suivantes past 400 lines), Aperçu for Markdown, Modifications with one patch per operation;
  a path outside the project is refused.
- Reasoning reads « Réfléchit »; added and removed lines are clearly green and red in both themes.

### Sub-agents in the thread, attachments copied per conversation

Automated: `crates/bricks-agent/tests/{agents,agents_runtime,admission_window}.rs`, `src/events.rs` and
`src/agents/runtime.rs` (tests), `src-tauri/src/services/{attachments,attachment_runs,run_records}.rs`
(tests), `tests/unit/{run-cost,conversation,agent-*}.test.ts`.

In the app, with a test project:

- A request that launches several sub-agents: one line each in the thread (dot, profile, description,
  duration), no `+N −N`; the second line follows the running tool; the dot stops pulsing at the end.
- A line opens the inspector: Résultat, Fichiers modifiés, Commandes open, Détails closed, no cost; a
  message sent to a running agent is answered by it; a message to an ended agent is refused.
- One « Coût de la réponse » line per response, provisional while sub-agents run, kept after a restart.
- No sub-agent list in the right panel; « Plus » is the last button of the top bar, after Lecteur.
- Environment: icons for project, folder and Git, no model or reasoning rows; Contexte is one bar.
- Paste, pick and drop a file and an image: chips show the copy; move or delete the original before
  sending: the message still carries it; the image is shown again after reopening the conversation;
  a PDF is refused at once.
- Window at 1440 × 900 and at its minimum 800 × 560, light and dark.
