# Bricks Desktop tests

## Running

```bash
cd apps/bricks-desktop
npm run check        # svelte-check + TypeScript
npm run test:unit    # Vitest (tests/unit)
npm run test:types   # type-check of the tests
cargo test -p bricks-desktop                       # Rust services, IPC, engine bridge
cargo test -p cersei-agent --test approval_override  # engine approval policy
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
