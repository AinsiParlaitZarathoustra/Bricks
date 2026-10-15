# Tests: Bricks Desktop

Owner: Sprint 14, Haiku 8 (settings and quality). Scope: `tests/**`. Product code is not covered
here beyond what the test files import.

## 1. Strategy

| Layer | What it covers | Where | Runs on |
| --- | --- | --- | --- |
| Rust unit and integration | IPC contract, services, engine bridge | `crates/**`, `src-tauri/src/**` (`cargo test -p bricks-desktop`) | macOS, Linux, Windows |
| Frontend unit (Vitest) | Pure logic: conversation reducer `Conversation.apply`, formatters | `tests/unit/*.test.ts` | any OS, CI |
| UI end-to-end (WebDriver) | Start path, project and conversation sidebar, real window | `tests/e2e/**` (WebdriverIO + tauri-driver) | **Linux and Windows only** |
| Manual UI checklist | Everything the WebDriver layer cannot drive on macOS | section 4 below | macOS |
| Accessibility and responsive review | Keyboard, focus, labels, contrast, narrow windows | `tests/REVIEW.md` | by reading the code, then by hand |

Decisions:

- **Logic is tested without components.** The reducer and the formatters are plain TypeScript, so
  they get fast unit tests. Component tests (Testing Library + jsdom) are not added yet: add them
  only for a component whose behaviour cannot be reached from the logic it uses.
- **macOS has no WebDriver for Tauri.** Tauri's WebDriver support (via `tauri-driver`) targets
  Linux and Windows. On macOS the WKWebView has no driver, so the sprint paths are checked by hand
  with the checklist below. Do not try to script the macOS app with AppleScript for regression
  coverage; it is brittle and does not test the webview.
- **No fixture in the product.** Test data lives in `tests/` only (`tests/unit/fixtures.ts`,
  `tests/e2e/seed.ts`). The app never reads it.

## 2. Running

Dev dependencies are **not** added to `package.json` by this sprint (see section 3). Once they are
installed:

```sh
cd apps/bricks-desktop
npx vitest run --config tests/vitest.config.ts     # unit tests
npx wdio run tests/e2e/wdio.conf.ts               # UI tests, Linux or Windows only
```

Suggested `package.json` scripts for the orchestrator:

```json
"test:unit": "vitest run --config tests/vitest.config.ts",
"test:e2e": "wdio run tests/e2e/wdio.conf.ts"
```

`npm run check` does not type-check `tests/` (its tsconfig only includes `src/`). Once `vitest`
and the WebdriverIO types are installed, add `tests/tsconfig.json` (extends the app config, includes
`tests/**`) so the test files are type-checked too.

### Unit tests (what they assert)

`tests/unit/conversation.test.ts` drives `Conversation.apply` with envelopes built by
`EnvelopeFeed` (`tests/unit/fixtures.ts`):

- text deltas stream into one assistant item, closed at run end;
- a tool call goes running, progress, then ok or error; a shell error is kept as command evidence;
  a tool left running is closed when the run ends;
- approval requested, then resolved (allow or deny) on the same timeline item;
- edit operations are summed per path within a run, and restart for each run;
- run outcome, final text, turns and duration (`run_finished.at - run_started.at`);
- a duplicate or older `seq` is ignored;
- sub-agent states (created, queued, running, completed), their tool calls and result; unknown agent ignored.

`tests/unit/settings-format.test.ts` covers the settings display helpers.

Last local run (through a scratch Vite runner, because Vitest is not installed yet): 19 tests passed,
0 failed. A negative control confirmed the matchers fail on wrong values.

### WebDriver tests (`tests/e2e`)

- `wdio.conf.ts` seeds a throwaway `BRICKS_HOME` (`seed.ts`: `desktop/projects.json` with one
  project folder), builds `bricks-desktop` with cargo, starts `tauri-driver` for each session.
- `specs/start-and-sidebar.e2e.ts` covers: the loading screen ends; the sidebar shows; the seeded
  project is listed and selected; the empty conversation state; the project menu opens; a
  conversation starts only when a provider is configured; F2 renames it.
- The native folder picker (« Nouveau projet ») cannot be driven by WebDriver. The spec does not
  click it; the project is seeded instead.
- Selectors use the accessible names of the navigation components. If those names change, update
  the spec in the same commit. Add `data-testid` only where no accessible name exists.
- Set `BRICKS_BINARY` to an existing build to skip the cargo build.

Linux prerequisites: `tauri-driver` (`cargo install tauri-driver`), `WebKitWebDriver` (package
`webkit2gtk-driver`), and a display (use `xvfb-run` on headless CI). Windows: `msedgedriver` matching
the installed WebView2 runtime. Check the official Tauri WebDriver page when first wiring this up:
the exact build flags can change between Tauri releases.

## 3. Dev dependencies to add (orchestrator; not added here)

Add to `apps/bricks-desktop/package.json` `devDependencies`:

- `vitest`: a release that supports Vite 8 (the project uses vite ^8.3).
- `@wdio/cli`, `@wdio/local-runner`, `@wdio/mocha-framework`, `@wdio/spec-reporter`, `@wdio/globals`:
  WebdriverIO v9 with the Mocha framework (the spec imports `$`, `browser`, `expect` from `@wdio/globals`).
- `tsx` only if the WebdriverIO TypeScript loader asks for it at first run.

No component-test packages yet. If a component test is needed later: `@testing-library/svelte` and
`jsdom`, plus `environment: 'jsdom'` for that file (`// @vitest-environment jsdom`).

## 4. Manual checklist (macOS)

Run on a debug build (`npm run tauri dev`, or the app bundle) with a configured provider in
`~/.bricks/providers.toml`. Use a scratch project folder, not one with work in it. Tick each item and
note the macOS version and window size.

**Start and projects**
- [ ] The loading screen « Démarrage de Bricks… » ends. Without providers, the « Aucun fournisseur configuré » state shows the providers file path.
- [ ] « Nouveau projet » opens the folder picker; the chosen folder appears in the list and is selected.
- [ ] Rename a project (menu, « Renommer… »), then cancel with Escape: the name does not change.
- [ ] Remove a project (« Retirer de Bricks »): the folder and its conversations stay on disk.
- [ ] A folder moved away shows « Dossier introuvable » and cannot be selected.

**Conversations**
- [ ] Create a conversation (« Nouvelle conversation »); the composer is focused.
- [ ] Rename by double-click and by F2; Enter saves, Escape restores the old title, blur saves.
- [ ] Search (titles and messages), then open a result: its conversation opens, and the project is selected first if it differs.
- [ ] Resume after restarting the app: the last conversation reopens with its history.
- [ ] Delete (« Supprimer »): the confirmation names the conversation; a running one warns it will be cancelled.

**Run, approvals, cancel**
- [ ] A prompt streams text progressively; the stop action is available while it runs.
- [ ] A tool needing approval shows the request with its preview; « Refuser » records « Refusé (par vous) » on the card and the tool does not run.
- [ ] Approve a tool: it runs and its output appears under the tool item.
- [ ] Cancel during a run: the run stops, the partial answer stays, and the text you had typed in the composer is still there.
- [ ] The final summary (outcome, duration, turns) shows at the end of the run.

**Composer**
- [ ] Change model during a run: the change says it applies to the next turn; it does not interrupt the run.
- [ ] Change effort and model between runs; the selection persists after switching conversations.
- [ ] Attach an image by paste, by drag and drop, and by the file picker; each shows a chip with a remove action.
- [ ] Attach a folder; remove an attachment before sending.

**Context and changes**
- [ ] The context ring reflects occupation (not cumulative usage); the « Contexte » card gives the figures and the capacity.
- [ ] Context after a model change is marked stale until the next measure.
- [ ] « Modifications »: switch between the tabs Conversation, Historique du projet and Git; the panel's open state is kept.
- [ ] A run that edits files lists them in its end card, with the sum of the edit operations; « Modifications » → Conversation lists the same files, and expanding a row shows its diff.

**Sub-agents**
- [ ] A spawned sub-agent shows its state (queued, running, completed, failed) and its tool calls.
- [ ] A sub-agent's result shows its summary without overwriting the main answer. States use the labels « En file d’attente », « En cours », « Terminé », « Échec », « Interrompu ».

**Multi-conversation**
- [ ] Start a run, switch to another conversation, come back: the run's text, approvals and timer are right.
- [ ] Run in two conversations at once: each keeps its own stream; no text leaks across.

**Terminal**
- [ ] Open the terminal; type, paste, resize the window: the output reflows.
- [ ] Ctrl-C interrupts a running command; the shell stays usable.
- [ ] `exit` closes the shell; the pane reports the end and can be reopened.

**Layout**
- [ ] Collapse and expand the left, right and terminal panes from the top bar (Tab to the toggle, Enter to press). The app defines no keyboard shortcut for them.
- [ ] Drag a separator to resize; the width survives a restart (« Interface » → « Dimensions » shows it).
- [ ] Settings → Interface → « Réinitialiser la disposition » restores the defaults at the next launch.

**Themes and zoom**
- [ ] Settings → Apparence: Clair, Sombre, Système. Système follows the macOS appearance live.
- [ ] Each theme keeps text legible: check the status colours (success, warning, error, running).
- [ ] Text scaling keeps every control reachable and nothing overlaps. The app defines no zoom shortcut (there is no menu in `src-tauri`), so use the macOS display setting « Agrandi » or the larger-text setting instead.

**Window sizes**
- [ ] Small (640 × 480, the minimum): both side panels become sheets; the composer and send action stay visible.
- [ ] Medium (about 1000 px wide): the right panel is a sheet; the left stays inline.
- [ ] Large (1280 px and up): all three columns inline; the centre keeps at least 480 px.
- [ ] Keyboard only, at each size: every action above can be reached with Tab and Enter; focus is always visible.

Record failures as issues with the steps, the macOS version, and the window size.
