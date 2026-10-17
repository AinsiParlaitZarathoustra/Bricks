# Tests: Bricks Desktop

Owner: Sprint 14, Haiku 8 (settings and quality). Sprint 14.1 (command mapping tests, regression
checks, checklist): Haiku 8, 2026-10-16. Sprint 14.2 (validation: approval override, providers editing,
preferences, tool lines, context, approval card, review): Haiku 4, 2026-10-16. Scope: `tests/**`,
`src-tauri/tests/*.rs` and `crates/cersei-agent/tests/approval_override.rs`.
Sprint 14.3 (validation): Haiku V, `tests/*-14-3.test.ts` and `src-tauri/tests/*_14_3.rs`; the review is in
`tests/REVIEW_14_3.md`. Product code is not
covered here beyond what the test files import. The review of the 14.2 Svelte and Rust changes is in
`tests/REVIEW_14_2.md`.

## 1. Strategy

| Layer | What it covers | Where | Runs on |
| --- | --- | --- | --- |
| Rust unit and integration | IPC contract, services, engine bridge, `/` registry mapping | `crates/**`, `src-tauri/src/**`, `src-tauri/tests/slash_mapping.rs` (`cargo test -p bricks-desktop`) | macOS, Linux, Windows |
| Frontend unit (Vitest) | Pure logic: conversation reducer `Conversation.apply`, `/` dispatcher, formatters | `tests/unit/*.test.ts` | any OS, CI |
| UI end-to-end (WebDriver) | Start path, project and conversation sidebar, real window | `tests/e2e/**` (WebdriverIO + tauri-driver) | **Linux and Windows only** |
| Manual UI checklist | Everything the WebDriver layer cannot drive on macOS | section 4 below | macOS |
| Accessibility and responsive review | Keyboard, focus, labels, contrast, narrow windows | `tests/REVIEW.md` | by reading the code, then by hand |

Decisions:

- **Logic is tested without components.** The reducer, the dispatcher and the formatters are plain
  TypeScript, so they get fast unit tests. Component tests (Testing Library + jsdom) are not added
  yet: add them only for a component whose behaviour cannot be reached from the logic it uses.
- **macOS has no WebDriver for Tauri.** Tauri's WebDriver support (via `tauri-driver`) targets
  Linux and Windows. On macOS the WKWebView has no driver, so the sprint paths are checked by hand
  with the checklist below. Do not try to script the macOS app with AppleScript for regression
  coverage; it is brittle and does not test the webview.
- **No fixture in the product.** Test data lives in `tests/` only (`tests/unit/fixtures.ts`,
  `tests/e2e/seed.ts`). The app never reads it.
- **`/` commands are tested at both ends.** `slash_mapping.rs` checks the wire value of every
  registry name and alias through the public `slash_parse`. `dispatch-coverage.test.ts` checks that
  the dispatcher gives every view and every engine command a desktop effect. Both read their lists
  from the source of truth (the registry, the generated bindings, `engine.ts`), so a new variant
  fails the suite until it is described here.
- **Event routing by session is not unit-tested.** The reducer does not check `session_id`; the app
  routes by it (section 5).

## 2. Running

```sh
# from apps/bricks-desktop
npm run test:unit                              # Vitest: tests/unit/*.test.ts
node_modules/.bin/tsc -p tests/tsconfig.json   # type-check the unit tests and the modules they import
npm run check                                  # svelte-check + tsc (src/ only)

# from the repo root
cargo test -p bricks-desktop --test slash_mapping   # `/` registry mapping through IPC
cargo test -p bricks-desktop --test providers_edit --test preferences_approval   # 14.2
cargo test -p bricks-desktop                        # all desktop Rust tests
cargo test -p cersei-agent --lib control::          # approval gate, settings, slash registry
cargo test -p cersei-agent --test approval_override # 14.2: the runtime approval override
```

`cargo test -p cersei-agent` without `--lib` or `--test` builds again since Sprint 14.3: the user copies
`crates/cersei-agent/tests/* 2.rs` that blocked it were removed (`docs/cleanup-14-3.md`). The problem is listed
in `REVIEW_14_2.md` I6.

`npm run check` does not type-check `tests/`: `tests/tsconfig.json` does (it extends the app config
and includes `tests/unit/**` and the state, dispatch and IPC modules they import).
`npm run test:e2e` is declared, but WebdriverIO is not installed yet (section 3).

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

`tests/unit/conversation-14-1.test.ts` covers the reducer fields added in Sprint 14.1:

- usage: one entry per response (`turnUsages`, tagged with its run); the session total is replaced,
  never summed (`usageTotalFull`); an unpriced total keeps its cost unknown;
- `run_usage` kept per root run; `search_results` (notes default to none, the latest search wins);
- `agent_profiles`; `skills` listing; trust request shown until resolved;
- memory: last recall; maintenance running from start to end; a cancelled maintenance is not running;
- jobs: start, byte counts, end state with exit code or signal and logs; events for an unknown job ignored;
- `context_cleared` (count, context marked stale, conversation kept); `compaction` (stale only when
  something was compacted); `skill_activated` and `skill_activation_failed` (a sub-agent's activation
  does not replace the session's; a failure shows an error notice); `agent_control_result`;
  `command_rejected`, cleared by the next run;
- session isolation: a stream for session A leaves session B unchanged, field by field; each
  conversation keeps its own `seq` (B's first envelope is not dropped because A is further along);
- one `it.fails` for the known gap in section 5.

`tests/unit/dispatch-coverage.test.ts` covers the `/` dispatcher (`features/commands/dispatch.ts`)
with a fake host:

- the 17 views of the generated `SlashPresent` binding each open their view or perform their action,
  with no engine call;
- the 15 engine `Command` types each have a desktop effect (sent, opened, kept locally, or refused),
  with and without an open session. The registry cannot produce `cancel` or `approve`; they are
  still handled;
- for every action, `submit` is sent only when the action is one (no `/` line falls through to the
  model), and the unknown-command fallback is never reached;
- an unknown view or engine type is refused and nothing is sent.

Other files in `tests/unit` (`commands-*.test.ts`, `composer-mentions.test.ts`) belong to other
Sprint 14.1 agents and are not described here.

`tests/unit/settings-format.test.ts` covers the settings display helpers.

`src-tauri/tests/slash_mapping.rs` (12 tests) covers the registry through `slash_parse` and `slash_commands`:

- every registry name and alias has a matrix row, and only those;
- each row's wire value is either `kind: engine` with the engine `Command` JSON (which the frontend
  forwards and the engine reads back as a `Command`), or `kind: present` with its view, or
  `Invalid` with the exact French message;
- an alias gives the same action, or the same error, as its name, for twelve arguments;
- `/resume <id>` and `/sessions <id>` never yield an engine `resume`;
- `/skill <name>` without a request is an error;
- `/effort default` sends `set_reasoning` with `profile: null`; `/search re:a.b` sends `regex: true`;
- `/changes <id>` (inspect, apply, discard) and `/jobs stop <id>` carry their ids;
- attachment paths keep spaces, accents and decomposed accents as typed (no normalisation);
- unknown and case-mismatched commands, lines without `/`, and the palette list match the registry.

Quirks pinned by the matrix, kept from the TUI (`docs/tui-inventory.md` section 6): `/search re:`
sends an empty regex; `/agents status` without an id is a profile query for the word `status`;
the `/changes` message still points at `changes ready`.

### Sprint 14.2 tests (validation)

Existing files that already cover the 14.2 changes (not repeated):

- `context-line.test.ts`: the line `≈ N / M tokens` for measured, counted, mixed and estimated
  provenance; a missing numerator gives `—`; an unknown window gives `—`, never the input limit; stale
  after a model change, a clear, or a compaction that compacted.
- `toolrow-line.test.ts`: states done, failed, running, interrupted (cancelled run); the duration in
  ms from `duration_ms`; the approval trace line.
- `settings-approval.test.ts`: the three approval options, in order, with French labels.
- `models-labels.test.ts`, `models-patch.test.ts`, `models-required.test.ts`: the models form.

New in Sprint 14.2:

- `toolline-render.test.ts` (7): server render of `ToolLine.svelte`: name, French state, ms duration
  only once finished; no arguments or output in the markup; no card, no button; decorative icons are
  `aria-hidden`. The Vitest run is slower than the others (about 15 s of transform, because the icons
  come from the `@lucide/svelte` barrel; REVIEW L6).
- `approval-render.test.ts` (5): server render of `ApprovalCard.svelte`: a pending request offers
  « Autoriser », « Autoriser pour la session » and « Refuser »; a resolved one is a single trace line
  (`Autorisé · Write · par vous`) with no button left.
- `validation-14-2.test.ts` (7): reducer facts behind those lines: no duration when the engine gave
  none; the line is built from the name only; a call cut by cancellation is interrupted; an approval
  resolved by the person gets its trace; a compaction that compacted nothing keeps the measure fresh;
  a new measure after a clear shows figures again.

Rust, Sprint 14.2:

- `crates/cersei-agent/tests/approval_override.rs` (7), scripted model through the real controller:
  the gate's refusals win over every mode (forbidden, explicit deny); `no_confirmation` allows at the
  gate without asking; `standard` keeps the rules and a session grant; `always_ask` ignores the grant
  and asks a read the rules allow; `no_confirmation` runs a write, and an explicit deny still refuses
  (the policy's message, nothing asked); each mode change is the next decision of the same open
  controller; a running sub-agent takes the change at its next decision, and the question it was
  waiting on is not answered by the change (`snapshot().pending_approvals` still has it). No built-in
  tool has the `Forbidden` level, so `Forbidden` is tested at the gate only.
- `src-tauri/tests/providers_edit.rs` (9), on a temporary `providers.toml`: an edit changes its fields
  and keeps every other byte (comments included); after a restart (a new registry from the same file)
  the renames and deletions are there and the header comment survives; reading the list writes nothing
  (a cancelled edit is a non-write); a refused edit writes nothing and its message names no secret; a
  stale revision is a conflict and the second writer must reload; a broken file is reported and never
  written over; a missing file is an empty list, and an edit does not create it; the views and errors
  carry no secret value; an unknown provider is `not_found`. The command glue (`apply_edit`) is
  `pub(crate)`: these tests run its steps (`commit`, a new registry, `DesktopCatalog::replace`).
- `src-tauri/tests/preferences_approval.rs` (5): each policy is saved with its wire value
  (`always_ask`, `no_confirmation`) and read back after a restart; the other preferences survive a save;
  an old file without the setting means `standard` and keeps its values; an unknown value (`yolo`,
  `no-confirmation`, `null`) never gives `no_confirmation`; a save that cannot be written changes nothing
  on disk.

Every Sprint 14.2 test writes only in `tempfile` folders; none reads or writes `~/.bricks`.

Last run (2026-10-16, Haiku 4, Sprint 14.2 validation; Haiku 8's 14.1 counts are superseded):

- `npm run test:unit`: 17 files, 318 passed (14 files, 299 before this sprint's additions).
- `npm run test:types`: no error.
- `npm run check`: 0 errors, 1 warning (`tsconfig.app.json`: `baseUrl` deprecation, already there).
- `svelte-autofixer` (read-only) on `ApprovalCard`, `ToolLine`, `ModelSettings`, `ModelRow`,
  `GeneralSettings`, `ProviderFormDialog`: no issue, no suggestion.
- `cargo test -p bricks-desktop`: lib 121 passed; `providers_edit` 9 passed; `preferences_approval` 5
  passed; `slash_mapping` 12 passed.
- `cargo test -p cersei-agent --lib control::`: 29 passed.
- `cargo test -p cersei-agent --test approval_override`: 7 passed, no warning.
- Not run: the negative controls (a mutated override) for the 14.2 tests; `cargo test -p cersei-agent`
  without a target (blocked, see above); anything on the macOS application (section 4, « non vérifié »).

Earlier run (Haiku 8, Sprint 14.1):

- `npm run test:unit`: 8 files, 225 passed, 1 expected failure (the `it.fails` known gap).
- `node_modules/.bin/tsc -p tests/tsconfig.json`: no error.
- `npm run check`: 0 errors, 1 warning (`tsconfig.app.json`: `baseUrl` deprecation, already there).
- `cargo test -p bricks-desktop`: lib 98 passed; `slash_mapping` 12 passed.
- Negative controls: changing one expected value in `slash_mapping.rs` fails the matrix test (then
  restored); changing the known-gap test to `it` fails with `expected 1 to be +0`.

### Sprint 14.3 tests (validation)

Existing files that already cover the 14.3 changes (read, not repeated):

- `terminal-isolation.test.ts` (three tabs with distinct output, rapid switches, inactive close,
  projects, detach keeps the shell, refit only when shown, fonts), `terminal-layout.test.ts` (the ratio
  bounds, the px to ratio migration of `terminal_height`).
- `theme-palette.test.ts`, `appearance-colors.test.ts` (palette rules, contrast, roles, reset),
  `theme-fonts.test.ts` (Geist and Geist Mono local, no Inter).
- `links-message.test.ts` (user-message links: punctuation, `&` in a query, `www.`, code, HTML-like text).
- `runend-summary.test.ts` (state, duration in ms, edit sums, binary files, reason and error).
- `models-profiles.test.ts`, `models-patch.test.ts` (profile drafts and list patches, the window never sent).
- `composer-skills.test.ts`, `settings-skills.test.ts` (skill choices, selection after a run, paging).
- `history-state.test.ts`, `adk-access.test.ts`, `changeset-links.test.ts` (history states, menu, links).
- Rust: `prefs.rs` tests (colours, ratio clamp, legacy file), `providers_toml.rs` tests (profiles,
  defaults, protected fields, context window kept), `commands/providers/tests.rs` (a saved free-named
  profile reaches the request it selects), `services/skills.rs` tests (listing, project skill listed but
  not trusted, reload, shadowing).

New in Sprint 14.3 (validation):

- `render-14-3.test.ts` (10): server render of `UserMessage.svelte` (links with trailing punctuation,
  `&` in a query, `www.`, no link in code, HTML-like text as text, `dir="auto"` and `text-start`) and of
  `RunEndCard.svelte` built from envelopes fed to `Conversation.apply` (« Terminé · 762 ms · +120 −24 »,
  no turn count, no model id, no « Durée : », binary files without counts, failed and cancelled states).
- `appearance-14-3.test.ts` (5): every variable the palette sets is declared in `:root` and `.dark` of
  `src/app.css`; every accent preset reaches 4.5:1 for its text in both modes; every background and panel
  preset keeps its text at 4.5:1.
- `leftovers-14-3.test.ts` (11 passed, 1 `it.fails`): static checks of the sources: the spec §6 texts are
  absent from the history tab and the run card; « Envoyer » and « Quelque chose à dire ? » are kept; the
  model form says « Nom API » with no window field; no Inter; the header keeps its three columns; raw HTML
  only in the sanitised Markdown path. The `it.fails` is an open defect (V2 below).
- `src-tauri/tests/profiles_14_3.rs` (10): the JSON the model form sends, read as a `ModelPatch` and
  written to a temporary `providers.toml`: a model edit keeps the context window and the profiles; a
  free-named profile with nested values and a removed key is read back after a restart and listed by the
  catalogue; duplicate ids, a non-object parameter block (array, number, string, null), a protected field
  (`messages`), a pointer without `/`, an unknown or removed default are refused, and the file is
  byte-for-byte unchanged; a removed default replaced or cleared in the same patch is written; a stale
  revision is a conflict and keeps the other writer's change.
- `src-tauri/tests/skills_prompt_14_3.rs` (4): the `submit` JSON the composer sends (text, file, folder and
  image blocks in order, skill names as chosen) reads back as the engine's `Command::Submit`; no skill
  field when none is chosen; a skill-only message; an unknown block kind is refused.

Not repeated here: the engine's own skill activation (refusal before anything is sent, trust) is tested in
`crates/cersei-agent`; the Svelte composer send path (draft kept on refusal) is read in `REVIEW_14_3.md`,
because it lives in `Composer.svelte` and has no pure function to test.

Last run (2026-10-17, Haiku V, Sprint 14.3 validation):

- `npm run check`: 0 errors, 1 warning (`baseUrl`, already there). `npm run test:unit`: 34 files, 510 passed,
  1 expected failure. `npm run test:types`: 1 error, V1 in `changeset-links.test.ts`; the new files pass.
- `npm run build`: ok (chunk-size warning only).
- `cargo build --examples -p cersei`, `cargo build --workspace`, `cargo test -p cersei-agent --no-run`,
  `cargo test --workspace --no-run`: ok, no warning.
- `cargo test -p bricks-desktop`: lib 146, `preferences_approval` 5, `profiles_14_3` 10, `providers_edit` 9,
  `skills_prompt_14_3` 4, `slash_mapping` 12, all passed.
- `cargo test -p cersei-tools -p cersei-types --lib`: 194 and 29 passed. `cargo test -p cersei-agent --lib`
  178 passed; `--test agents_runtime` 21 passed.
- Not run: the macOS application, the full `cersei-agent` test set, negative controls.

### Sprint 14.4 tests (validation)

Owner: Haiku 3 (Sprint 14.4, tests and regression review). Files:

- `tests/unit/session-flow-14-4.test.ts` (15 tests): « Nouvelle conversation », the single conversation
  start, the per-project drafts, and the `/` dispatch wired to the app store.
- `tests/unit/run-labels-14-4.test.ts` (9 tests): command labels on user messages, live and after a reopen.
- `src-tauri/tests/run_labels_14_4.rs` (8 tests): the label store across a restart, and the wire shapes.

`tests/unit/drafts-labels-14-4.test.ts` belongs to another agent and is not changed here.

How they run. The unit tests replace `invoke` of `@tauri-apps/api/core` with a recording fake, so the real
IPC client (`src/lib/ipc/client.ts`) runs and each `session_open` and `session_command` is counted at the
boundary. Each test loads a fresh app store and uses fake timers, so the debounced preference save never
runs between tests. The Rust test uses only the label store on a temporary home: it opens no conversation.

What is pinned:

- « Nouvelle conversation » called 20 times: no IPC call, one draft object, composer focus +20.
- A composer send and a dispatch submit started together share one `session_open`; the conversation takes
  the project draft (text, attachments, skills) and its model and effort.
- Drafts stay per project through a switch, an opened conversation and a new one; a conversation created in
  B takes the draft of B only.
- `/skill debug fix it` with no conversation: one creation, then one `session_command` with
  `display: '/skill debug'` and `prompt.skills: ['debug']`; with a conversation open, no second session.
  `/search`, `/clear`, `/compact` and `/cancel` with no conversation are refused with no session. Views,
  pickers and `/model` open no session.
- Labels: a `run_labeled` before or after its `run_started` gives one labelled message; a run without a label
  has no `commandLabel` key; a reopened session takes its labels from `run_labels`; a session stored before
  labels opens with none; a running run is labelled once by its run id, and a repeated label changes nothing.
- Rust: labels, deletions and a write cut short by a crash survive a restart of the store on the same home;
  a label already stored is not stored again after a restart; the `run_labeled` JSON and an `OpenedSession`
  without `run_labels` read as the frontend expects.

Expected failures. These three are product defects. They stay red (plain `it`, not `it.fails`) until the
owner fixes them, so `npm run test:unit` reports 3 failed:

- **S14.4-1, labels move to the wrong message on reopen.** `src/lib/state/conversation.svelte.ts:400`
  (`free.findIndex((i) => i.text === label.prompt)`) gives the label to the first unlabelled message with the
  same text, so `/skill debug Analyse` labels an earlier plain « Analyse ». Test: `run-labels-14-4.test.ts`,
  « an earlier message with the same text … ». Fix: store the position of the user message in `RunLabel`
  (the number of user messages before it, set in `engine/live.rs` at `RunStarted`) and match on it; keep the
  text match only for labels without a position.
- **S14.4-2, a « lagged » event drops the unsent draft of the open conversation.** `app.svelte.ts:471-472`
  deletes the conversation and reopens it with an empty draft. Test: `session-flow-14-4.test.ts`, « survives
  a « lagged » reopen ». Fix: pass the draft to the reopen, `this.open(conv.projectId, e.session_id, undefined,
  undefined, conv.draft)`.
- **S14.4-3, a conversation created for A is shown after the user moved to B.** `app.svelte.ts:374` calls
  `show()`, which sets `currentProjectId` (`app.svelte.ts:383`) when the creation completes. Test:
  `session-flow-14-4.test.ts`, « not shown when the user has moved to project B ». Fix: show a created
  conversation only while its project is still the current one.

Test-environment defect, blocking `cargo test -p bricks-desktop`:

- **S14.4-4, the lib tests write sessions into the real `~/.bricks/sessions`.** `src-tauri/src/engine/sessions.rs:62`
  builds the engine config with `bricks_config::engine_config`, which points the session folder at the global
  `bricks_home()/sessions`, not at the `AppState` home. Two tests in `src-tauri/src/commands/inspect.rs` open a
  session with a temporary home and so write there on every run: `the_configuration_never_carries_a_provider_key`
  (line 600) and `an_open_session_is_located_by_its_controller` (line 717). Fix: add
  `cfg.sessions_dir = self.sessions_dir().to_path_buf();` after line 62. Production is unchanged, since
  `AppState::new` already uses `bricks_home()`. Until it is fixed, run the Rust tests of this sprint with
  `cargo test -p bricks-desktop --test run_labels_14_4`, not the whole package. Each run leaves two folders
  of the form `<date>-<time>-<id>.files` holding only `session.json`; they are not deleted here.

Not covered:

- The Composer send path, the slash host and the draft settlement in `Composer.svelte`: there is no component
  test harness. `session-flow-14-4.test.ts` runs the composer's `ensureSession` logic against the app store.
- The engine's label queue in `engine/live.rs` (the `Expected` queue and `ingest`) needs a running controller:
  it is covered only by reading the code.
- The restart test does not go through a real engine session.

Last run (2026-10-17, Haiku 3):

- `npm run test:unit`: 38 files, 555 passed, 3 failed (S14.4-1 to S14.4-3, as above).
- `npm run test:types`: no error.
- `cargo test -p bricks-desktop --test run_labels_14_4`: 8 passed.
- Not run: the full `cargo test -p bricks-desktop` after the leak was found (S14.4-4). Its first run, before
  the leak was reported, passed: lib 157, `preferences_approval` 5, `profiles_14_3` 10, `providers_edit` 9,
  `skills_prompt_14_3` 4, `slash_mapping` 12.

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

## 3. Dev dependencies

- `vitest` is in `devDependencies` (5.0.5 installed); `test:unit` runs.
- WebdriverIO is **not** installed. To run `test:e2e`, add to `apps/bricks-desktop/package.json`
  `devDependencies`: `@wdio/cli`, `@wdio/local-runner`, `@wdio/mocha-framework`, `@wdio/spec-reporter`,
  `@wdio/globals` (WebdriverIO v9 with Mocha; the spec imports `$`, `browser`, `expect` from
  `@wdio/globals`). Add `tsx` only if the WebdriverIO TypeScript loader asks for it at first run.
  This is an orchestrator change; it was not made here.

No component-test packages yet. If a component test is needed later: `@testing-library/svelte` and
`jsdom`, plus `environment: 'jsdom'` for that file (`// @vitest-environment jsdom`).

## 4. Manual checklist (macOS)

Run on a debug build (`npm run tauri dev`, or the app bundle) with a configured provider in
`~/.bricks/providers.toml`. Use a scratch project folder, not one with work in it. Tick each item and
note the macOS version and window size.

**Status.** Nothing in this section was run on the macOS application for Sprint 14.1. Every item,
the existing ones included, is « non vérifié » until someone ticks it on a real build. Items in
the tables below carry the status explicitly.

**Retired controls.** The panel switches, the pixel dimensions and « Réinitialiser la disposition »
are no longer in Settings. No item checks them; the « Général » item below checks that they are gone.

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

**Composer**
- [ ] In the normal state the placeholder reads exactly « Quelque chose à dire ? » (« non vérifié »). It carries no text about Entrée or Maj+Entrée.
- [ ] Other states keep their messages (code: `Composer.svelte`, « non vérifié »): no project « Créez un projet pour écrire. »; folder missing « Le dossier du projet est introuvable. »; no model « Aucun modèle configuré. »; while a run goes « Préparez le message suivant… ».
- [ ] Enter sends; Maj+Entrée adds a line; Cmd+Entrée sends too (« non vérifié »).
- [ ] During a run, Enter does not send a prompt: the draft stays. A `/` command still runs, for example `/cost` (« non vérifié »).
- [ ] Change model during a run: the change says it applies to the next turn; it does not interrupt the run.
- [ ] Change effort and model between runs; the selection persists after switching conversations.
- [ ] Attach an image by paste, by drag and drop, and by the file picker; each shows a chip with a remove action.
- [ ] Attach a folder; remove an attachment before sending.

**Palette `/` (keyboard only, « non vérifié » for every item)**
- [ ] Type `/` at the start of the composer: the « Commandes » list opens above it. Nothing is sent.
- [ ] ↓ and ↑ move the selection; Tab completes the highlighted name.
- [ ] Enter on an incomplete name completes it; Enter on a complete name runs the command.
- [ ] Escape closes the list and keeps the draft exactly as typed.
- [ ] `/nope` shows a French error (« commande inconnue /nope (essayez /help) »); the draft stays.
- [ ] `/help` (or `/?`) opens « Commandes » from the same list as the palette.

**Commands migrated from the TUI (Sprint 14.1, « non vérifié » for every row)**

Expected effects come from `docs/sprint-14-1-ui.md` §5 and `docs/tui-inventory.md`. Run each command
in a scratch conversation; use a controlled provider where the answer depends on the model.

| Command | Check | Expected observable effect |
| --- | --- | --- |
| `/model`, `/m` | Type `/model` | The model picker opens, the current model is marked. |
| `/model <provider/model> [profile]` | Type the full form | The model shown changes; an unknown reference is refused with a message. |
| `/effort`, `/effort <profile>`, `/effort default` | Pick a profile, then `default` | The effort picker opens; `default` restores the model's default; an unknown profile is refused by the engine. |
| `/search <text>`, `/s <text>` | Search a word that exists in the project | The « Recherche dans le code » view lists file and line hits, with the elapsed time. Conversation search is a different thing. |
| `/search re:<regex>` | A valid regex, then an invalid one | Hits for the valid regex; the invalid one shows the engine's error. |
| `/search` (no text) | Type it alone | Refused: « donnez un texte à rechercher ». |
| `/agents` | Open the catalogue, search a word, `/agents reload` | « Profils d’agents » lists profiles (20 per page) with diagnostics; reload re-reads the files. |
| `/agents running` | While a sub-agent runs | « Sous-agents » lists this session's instances only. |
| `/agents status\|result\|cancel <id>` | On a running sub-agent | The instance view opens; `cancel` really stops the agent, which ends in a cancelled state. |
| `/skills` (`<words>`, `page n`) | Open the catalogue, page 2 | « Skills » lists sources and states, 20 per page. |
| `/skills reload` | After editing a skill file | The listing reflects the file on disk. |
| `/skills trust` (`allow`), `/skills deny` (`refuse`) | In a project with untrusted skills | The decision is recorded for the session; nothing is trusted automatically. |
| `/skill <name> <request>` | A valid name, then an unknown name | Valid: the request is sent with the skill. Unknown: nothing is sent, a French error shows, the draft stays. |
| `/changes <id> inspect` | On a sub-agent's ChangeSet | The « Modifications isolées » view shows the patch. |
| `/changes <id> apply` | On a ChangeSet that conflicts with the tree | A conflict writes nothing: check the working tree before and after. |
| `/changes <id> discard` | On a ChangeSet | After the confirmation, the ChangeSet is discarded and no file changes. |
| `/jobs` | While a background job runs | « Tâches de fond » lists this session's engine jobs, separate from terminals. |
| `/jobs stop <id>` | On a running job | The job stops (graceful, then forced); its final state shows. |
| `/memory` | Memory on and off | « Mémoire » shows the space, the last recall and the maintenance state; with memory disabled it says so, and gives no space. |
| `/context`, `/ctx`, `/context details` | Before and after a message | « Contexte » shows occupation and capacity; estimates are marked ≈; `details` adds provenance, margins and notes. |
| `/cost`, `/usage`, `/cost details` | After two answers | « Consommation » shows the session totals, once; `details` gives one row per response; an unpriced model shows as not priced, with no invented cost. |
| `/session` | In any conversation | « Session » shows id, title, folder, model and effort, activity and memory space. |
| `/resume`, `/sessions` | With several sessions in this project | « Reprendre une session » lists them. |
| `/resume <id>` | A session from another project | Its project is selected and the conversation opens with its history, in its own folder. An unknown id shows a French refusal and the draft stays. |
| `/compact` | During a run, then idle | During a run: refused with a message. Idle: the context is summarised; the progress and result show. |
| `/clear` | In a conversation with history | « Vider le contexte » explains the scope and asks first; after confirming, the active context is empty and the conversation stays in the list and on screen. |
| `/diff` | After an edit run | The changes panel opens: proposed, applied in this session, and the git diff. |
| `/tools` | In any conversation | « Outils » lists tools, permission level and first line of description; no secrets. |
| `/mcp` | With no MCP server configured | « Serveurs MCP » says that no server is configured; there is no install screen. |
| `/config` | In any conversation | « Configuration » shows the effective settings and approval rules; no key or auth header. |
| `/file <path>` | A path with spaces and accents, quoted: `"Mon dossier/é.txt"` | A file chip is added to the draft and sent with the next message. A missing file is refused. |
| `/folder <path>` | An existing folder | A folder chip is added (bounded listing). |
| `/image <path>`, `/img <path>` | A PNG; then a model without image input | An image chip is added. A model that does not accept images refuses it with a message (per the TUI inventory). |
| `/help`, `/?` | Open help | « Commandes » lists the commands from the registry and the desktop keys. |
| `/quit`, `/exit`, `/q` | See the « /quit » group below | As described there. |

Each row above is « non vérifié ».

**`@` mentions and input history (« non vérifié »)**
- [ ] Type `@` in the composer: a file list opens. ↓ and ↑ choose, Tab or Enter attaches, Escape closes, Ctrl+R refreshes the list.
- [ ] An attached mention leaves the typed text and appears as a chip; the chip's kind matches the file (image, folder or file).
- [ ] In an empty composer, ↑ recalls the last prompt of this conversation; ↓ goes forward again and returns the draft that was there before.
- [ ] A draft typed before ↑ is never overwritten: it comes back at the end of the browsing.
- [ ] Recalled entries stay with the conversation they came from: switch conversations and back.

**Tool approval, cancel, resume (« non vérifié »)**
- [ ] A prompt that makes the agent run a tool needing approval shows the request with its preview. Allow: the tool runs and its output appears under the tool item.
- [ ] Reject the same kind of request: the card becomes one line « Refusé · Write · par vous » (the tool name is in it) and the tool does not run.
- [ ] Cancel during a tool run: the run stops, the partial answer stays, and the draft typed meanwhile is still there.
- [ ] Restart the app, then resume the conversation: its saved history is there (« non vérifié »).
- [ ] `/resume <id>` from another project: see the table above.

**`/quit` (« non vérifié »)**
- [ ] `/quit` with no run active closes the application.
- [ ] `/quit` during a run opens « Quitter Bricks ? », listing the conversations with a run in progress. « Rester » closes the dialog and changes nothing.
- [ ] « Quitter » closes the application. Code path: `app_quit` settles and closes every open session (`engine::shutdown`), then stops the terminals. After relaunch, the conversation shows what was saved; check whether the partial answer of the cancelled run is there (not verified). Other conversations are not stopped abruptly.

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
- [ ] A `/` command typed in one conversation (for example `/compact`) acts only on that conversation (« non vérifié »).

**Terminal**
- [ ] Open the terminal; type, paste, resize the window: the output reflows.
- [ ] Ctrl-C interrupts a running command; the shell stays usable.
- [ ] `exit` closes the shell; the pane reports the end and can be reopened.

**Layout**
- [ ] Collapse and expand the left, right and terminal panes from the top bar (Tab to the toggle, Enter to press). The app defines no keyboard shortcut for them.
- [ ] Drag a separator to resize; the width survives a restart (compare the panel width before quitting and after relaunching).
- [ ] Settings → Général shows only the one-line note that the layout is remembered: no switch, no pixel value, no reset button.

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

**Sprint 14.2 (« non vérifié » for every item)**

- [ ] Context: the line reads `≈ N / M tokens`; after a model change it reads `— / — tokens` until the
  next measure; a model with no known window shows `—` (not the input limit).
- [ ] Tool lines: each shows the tool name, the state (En cours, Terminé, Échec, Interrompu) and the
  duration in ms once finished; no command, path or output is in the line. Cancel a run during a tool
  call: that call reads Interrompu.
- [ ] Approval card: a pending request shows « Autoriser », « Autoriser pour la session », « Refuser »;
  once answered it is one line. Under « Toujours demander » the session button has no effect (REVIEW M4).
- [ ] Settings → Général → « Approbation des outils »: three choices, each with its one-line effect. Pick
  « Sans confirmation », then run a write: it runs without a question. Pick « Toujours demander »: the
  next write is asked. Restart the app: the choice is still there.
- [ ] « Paramètres » in the left panel footer opens Settings in the narrow Sheet and in the wide layout.
  With the left panel collapsed on a wide window, it is not reachable (REVIEW M2).
- [ ] Settings → Modèles: rename a provider and change a model's limit; the Enregistrer result shows in
  the list. « Annuler » closes the dialog and the providers file is byte-for-byte unchanged.
- [ ] Delete a model: the confirmation names it. Delete a provider: the confirmation lists its models.
- [ ] Edit the providers file by hand to an invalid state: Settings shows the message and no secret
  value. Fix the file: the list is back only after closing and reopening Settings (REVIEW L3).
- [ ] Pencil and trash have a tooltip and an accessible name (« Modifier le fournisseur … »,
  « Supprimer le modèle … »); the trash icon is red.
- [ ] Keyboard only: Tab reaches the footer button, the approval buttons, each model row (Enter expands
  it), and the dialogs; Escape closes a dialog without writing.
- [ ] Composer: the placeholder reads « Quelque chose à dire ? »; `/` opens « Commandes »; `@` opens the
  file list.

**Sprint 14.3 (« non vérifié » for every item until someone ticks it on a real build)**

Appearance and header
- [ ] Settings › Apparence: pick an accent in light mode, close and reopen the app: the colour persists; switch to dark: the dark value is the one saved for dark (not the light one).
- [ ] « Réinitialiser : Accent » returns the accent to the theme token; the system mode applies the set of the resolved mode.
- [ ] Geist renders in the interface and in the terminal; accents, long lines and xterm columns look right after the first load.
- [ ] Title and project are centred at the top; the centre stays put with few and with many buttons on the right; a long title truncates; the left toggle sits at the top left; the window drags from empty header areas.

Composer and links
- [ ] The send tooltip reads « Envoyer »; the placeholder reads « Quelque chose à dire ? ».
- [ ] Type Arabic or Hebrew, then French with an English word: the textarea and the user bubble align by their first strong character.
- [ ] A user message with `https://exemple.fr/?a=1&b=2.` opens the system browser; the webview does not navigate; a `www.` address opens too; an address in backticks is code.
- [ ] A message with `<img src=x onerror=…>` shows the text and runs nothing.
- [ ] `+` menu › Skills: choose two skills, see the removable chips, send with an attachment: the run starts with the skills; remove a chip before sending: no file of the skill is touched.
- [ ] A skill refused by the engine (for example a project skill not approved) keeps the draft, the attachments and the chips.

Run card and history
- [ ] A finished run shows « Terminé · N ms · +a −d » (no turn count, no model, no « Durée : »); a cancelled run shows « Annulé » only; a failed run shows its error.
- [ ] Modifications › Historique: an empty project shows nothing under the tab; entries show directly; a failed load keeps « Réessayer ».

Terminals
- [ ] Three terminals in one project: each keeps its own output; switching quickly shows only the chosen one; closing an inactive tab leaves the shown one alone.
- [ ] Switching projects brings back each project's terminals with their state.
- [ ] Drag the terminal divider, quit and reopen: the proportion comes back; a small window keeps the composer, the tabs and the terminal usable.
- [ ] The shell's size follows the visible terminal (`stty size` in the shell after a resize).

Models and profiles
- [ ] The model form shows « Nom API », « Entrée maximale (tokens) », « Sortie maximale (tokens) » and no context window field; a saved change keeps the window (the read-only row still shows it).
- [ ] Add a profile with a free id and JSON parameters; an invalid JSON keeps the form and writes nothing; a duplicate id is refused; removing the default asks for a new choice.
- [ ] The saved profile appears in the composer's effort choice after the save. The provider's answer to a free parameter is not checked here (« non vérifié »).

Skills and ADK access
- [ ] Settings › Skills lists the personal and built-in skills; search, « Recharger », and the project source (listed, not trusted) work.
- [ ] Panneau droit › Plus lists each view once; each opens; stopping a sub-agent or a job asks for confirmation.

## 5. Known gaps and limits

- **Session routing (open).** `app.svelte.ts` sends each envelope to `conversations.get(env.session_id)`.
  That routing needs the Tauri event listener, so it is not unit-tested. The reducer itself does not
  compare `env.session_id` with its own session: an envelope of another session applied directly
  changes the conversation. The test `a conversation ignores an envelope that belongs to another
  session` in `conversation-14-1.test.ts` is `it.fails` for this reason. Fix: compare the ids in
  `Conversation.apply` (orchestrator-owned file), then change `it.fails` to `it`.
- **Rust visibility.** `parse_line`, `to_ipc` and `command_list` in `commands/slash.rs` are `pub(crate)`,
  so the integration test reaches the registry only through `slash_parse` and `slash_commands`. Their
  own tests stay in that module.
- The terminal interface (`bricks-tui`) was removed in Sprint 14.1; its command registry lives in
  `cersei_agent::control::slash` (tests: `cargo test -p cersei-agent --lib control::slash`).
- **WebDriver** tests cannot run on macOS and WebdriverIO is not installed (section 3).
- Windows and Linux were not validated in Sprint 14.1.
- **Sprint 14.2 open items** (details in `tests/REVIEW_14_2.md`, not fixed here: they are product code):
  - M1: the approval preference is shown before it is saved, with no rollback on failure (`app.svelte.ts`).
  - M2: Settings is reachable only from the left panel footer.
  - M3: after a write conflict the dialog keeps the old values, so a retry can overwrite a concurrent change.
  - M4: a session grant given under « Toujours demander » is recorded and applies again after the mode is
    changed back (`approval.rs`); the session button is shown in that mode with no effect. A decision is
    needed; the tests do not pin the current behaviour.
  - L1: an unknown `tool_approval` value makes the whole preferences file fall back to defaults.
  - `cargo test -p cersei-agent` without a target builds again (Sprint 14.3, section 2); the `… 2.rs` copies are gone.
- **Sprint 14.3 open items** (details and fixes in `tests/REVIEW_14_3.md`; product code, not changed here):
  - V1: `tests/unit/changeset-links.test.ts:17` does not type-check (`npm run test:types`, TS2741): the `{}`
    operation has no `changesetId`. Test-only fix: give it `{ changesetId: null }`.
  - V2: `ConversationChanges.svelte:75-83` (Conversation tab) still shows « Aucune modification » and the
    permanent « Chiffres : somme des opérations… » sentence that spec §6 removes from the history. The
    `it.fails` in `leftovers-14-3.test.ts` turns into `it` once fixed.
  - V3 to V5: permanent explanatory texts in `SkillsSettings.svelte:117`, `ModelFormDialog.svelte:265` and
    `AppearanceSettings.svelte:88` (spec §5, §6 and §9 ask for none).
  - V6, V7: heading comment in `ConversationTitle.svelte:60` (it is an h2, the page h1 is the sr-only
    « Bricks »); unnamed icon button in `components/ui/sidebar/sidebar-trigger.svelte`, which nothing imports.
  - Native behaviour (section 4, Sprint 14.3 items) is not verified: the macOS application was not started.
