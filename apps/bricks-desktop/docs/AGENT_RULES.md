# Sprint 14 — rules for every Bricks Desktop agent

Repository: /Users/maximebuffet/Documents/bricks, branch `Bricks_Desktop`. App: `apps/bricks-desktop/`
(Svelte 5 + TS + Vite + Tailwind 4 + shadcn-svelte in `src/`, Tauri 2 in `src-tauri/`).

1. **First line of your final report**: the exact model id you are running as and your effort level
   (from your system prompt), then the list of Svelte MCP calls you made (tool + topic/file) if you
   touched Svelte.
2. **Only edit the files/directories you are assigned.** Never touch `package.json`, lockfiles,
   any `Cargo.toml`, `src-tauri/src/lib.rs`, `src-tauri/src/contract.rs`, `src/App.svelte`,
   `src/lib/ipc/engine.ts`, `src/lib/state/` unless your brief says you own it. Need a dependency or
   a contract change? Stop and say so in your report (exact change + reason); do not work around it.
3. **Never** `git add`, `git commit`, `git stash`, `git checkout`, `git clean`, or delete files you did
   not create. Pre-existing files named `… 2.rs` / `… 2.md` are user data: ignore them, don't touch.
4. **No fake data in the product**: no mock conversations, simulated agents, invented stats,
   or buttons that do nothing. Fixtures stay in tests. If a capability is missing, show an honest
   "Unavailable"/error state.
5. **Svelte work** (Svelte 5 runes, TypeScript): load the official Svelte MCP tools with
   `ToolSearch` query `svelte` (tools `mcp__svelte__list-sections`, `mcp__svelte__get-documentation`,
   `mcp__svelte__svelte-autofixer`). Call `list-sections`, then `get-documentation` for relevant
   topics, and run `svelte-autofixer` on every component you create or substantially change; fix and
   re-run until clean. Then run `npm run check` in `apps/bricks-desktop` (fix errors in your files).
   Use shadcn-svelte components from `$lib/components/ui/*` (already installed: sidebar, resizable,
   scroll-area, collapsible, dropdown-menu, dialog, alert-dialog, tooltip, button, input, textarea,
   separator, badge, select, popover, command, sonner, skeleton, tabs, switch, label, card, kbd,
   sheet, input-group) and icons from `@lucide/svelte`. All user-facing text in **French**, app name
   **Bricks**.
6. **Rust work**: `cargo build -p bricks-desktop` and `cargo test -p bricks-desktop` from the repo root
   must pass for your modules. Blocking/Git/IO work runs off the UI thread (`tokio::task::spawn_blocking`
   or async). Bound buffers. Error type: `contract::IpcError`.
7. The IPC contract is `src-tauri/src/contract.rs` + the command list in `src-tauri/src/lib.rs`
   (signatures in `src-tauri/src/commands/*.rs`, `git/commands.rs`, `pty/commands.rs`). Command
   argument names are fixed by those signatures (Tauri maps `snake_case` Rust args to `camelCase` JS keys).
8. Final report (concise): model/effort, files written, what works, checks you ran with real results,
   MCP calls, known limits. Do not claim anything you did not run.

## Frontend wave (read before coding)
- Global state: `src/lib/state/README.md`, `app.svelte.ts`, `conversation.svelte.ts` (orchestrator-owned:
  read, don't edit; if you need a new field/method, report the exact need, and meanwhile derive locally
  in your feature from existing fields).
- IPC: `src/lib/ipc/client.ts` (typed functions), engine JSON types `src/lib/ipc/engine.ts`. Generated
  `src/lib/ipc/bindings/*` may still be missing while Haiku 1 finishes: errors from those imports in
  `npm run check` are expected until then — make YOUR files error-free otherwise.
- Layout/design: `src/lib/layout/README.md` (AppShell, TopBar, EmptyState, SectionCard, StatusDot,
  DiffStat) and tokens in `src/app.css` (`diff-add`, `diff-del`, `success`, `warning`, `info`,
  `code-bg`...). Use Tailwind token utilities, no hard-coded colours.
- Entry points are already stubbed and mounted by `src/App.svelte` (orchestrator). Keep their file
  names and props exactly; replace the stub body.
- Long lists: avoid re-rendering the whole conversation per streamed delta (keyed `{#each}`, derived
  per item, throttle markdown re-render of the streaming item ~50–100 ms).
- Untrusted content (messages, tool output): never `{@html}` without DOMPurify sanitizing; no scripts.
- Effort note for your report: you are launched with effort `xhigh` by the orchestrator; state the
  model id you see in your system prompt.
