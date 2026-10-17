# Review of Sprint 14.3 (validation)

Reviewer: Haiku V (claude-haiku-5-5). Effort: not stated in my system prompt. Base: `e62a6be` plus the
uncommitted 14.3 changes in the working tree. Scope: read-only on product code. New files are
`tests/unit/*-14-3.test.ts` and `src-tauri/tests/*_14_3.rs`, plus this file and the README sections.

Svelte MCP calls: `ToolSearch` (`svelte`), then `svelte-autofixer` (read-only) on
`ConversationChanges.svelte`, `ColorPicker.svelte`, `ConversationTitle.svelte`. No issue reported; the
suggestions are the `$effect` loaders that call `untrack` on purpose. No Svelte file was written, so
`list-sections`/`get-documentation` were not needed.

## 1. Verdict

Compatibility, profiles, models, skills transport, rendering and appearance wiring pass their checks.
Three defects need an owner. The one that blocks a check is V1: `npm run test:types` fails on a test
file that predates this validation. V2 is the only spec-level gap (text the spec removes is still shown
in the Conversation tab). V3 to V7 are low-severity wording and accessibility items.

Not verified: the macOS application was not started. Every native item (section 4 of
`tests/README.md`, « Sprint 14.3 ») is still « non vérifié ».

## 2. Checks run

| Command | Result |
| --- | --- |
| `npm run check` | 5028 files, 0 errors, 1 warning (`tsconfig.app.json` `baseUrl` deprecation, already there). `tsconfig.node.json` ok. |
| `npm run test:unit` | 34 files, 510 passed, 1 expected failure (`it.fails`, V2). Before this sprint's validation files: 31 files, 484 passed. |
| `npm run test:types` | **1 error**: `tests/unit/changeset-links.test.ts(17,49)` TS2741 (V1). The new `*-14-3` files type-check. |
| `npm run build` | ok. Chunk-size warning only. Geist woff2 emitted as hashed assets; no external font or CDN reference in `dist`. |
| `cargo build --examples -p cersei` | ok (facade examples compile). |
| `cargo build --workspace` | ok, 0 warnings. |
| `cargo test -p cersei-agent --no-run` | ok, 0 warnings, 29 test executables (no target filter needed; the `… 2.rs` copies are gone). |
| `cargo test --workspace --no-run` | ok, 0 warnings. |
| `cargo test -p bricks-desktop` | lib 146 passed; `preferences_approval` 5; `profiles_14_3` 10 (new); `providers_edit` 9; `skills_prompt_14_3` 4 (new); `slash_mapping` 12. All passed. |
| `cargo test -p cersei-tools -p cersei-types --lib` | 194 and 29 passed, 1 ignored in each (not examined). Includes the `git_utils` tests added by the cleanup. |
| `cargo test -p cersei-agent --lib` and `--test agents_runtime` | 178 and 21 passed. The full `cersei-agent` test set was not run (only `--no-run`). |

## 3. Coverage of spec §11

| Spec item | Evidence | Status |
| --- | --- | --- |
| SDK paths and signatures kept | workspace and facade builds, test builds, cersei-tools/types/agent tests above | passed |
| Profiles: free names, nested values, saved, reloaded, transmitted | `commands/providers/tests.rs` (existing, in-crate, real HTTP body); `profiles_14_3.rs` (new, after restart, catalogue) | passed |
| Profiles: invalid JSON object, duplicates, default removed, file unchanged | `providers_toml.rs` tests; `profiles_14_3.rs` (new, through `commit`, byte-for-byte) | passed |
| Model edit keeps the window | `models-patch.test.ts` (form sends `null`); `providers_toml.rs`; `profiles_14_3.rs` (new) | passed |
| Skills: catalogue, project not trusted, reload | `services/skills.rs` tests (in-module, temp dirs) | passed |
| Skills: `Prompt.skills` with text and attachments | `skills_prompt_14_3.rs` (new, wire JSON to engine `Command`); `composer-skills.test.ts` (helpers) | passed (wire); send path by reading only, see N3 |
| Terminals: three tabs, rapid switches, inactive close, projects, ratio migration | `terminal-isolation.test.ts`, `terminal-layout.test.ts` (existing); no gap found, nothing added | passed |
| Links: punctuation, `&`, `www.`, code, HTML-like text, `dir="auto"` | `links-message.test.ts` (existing); `render-14-3.test.ts` (new, server render of `UserMessage`) | passed |
| RunEndCard line from real run data | `render-14-3.test.ts` (new, envelopes through `Conversation.apply`) | passed |
| Colours per mode, roles, contrast; Geist; no Inter | `theme-palette`, `appearance-colors`, `theme-fonts` (existing); `appearance-14-3.test.ts` (new: palette variables declared in `:root` and `.dark`, accent and surface presets readable) | passed |
| Leftover texts, header structure, « Envoyer », a11y names | `leftovers-14-3.test.ts` (new); code review below; autofixer on 3 files | V2 open; others passed |
| Native: colours persist, Geist loaded, centring, collapse at left, links open without navigation, RTL, run card, tabs, PTY size | none run on the app | **not verified** |

## 4. Defects

Severity: High blocks a required check; Medium is a spec gap; Low is wording or accessibility.

| ID | Sev | Owner (file) | Where | Problem | Fix |
| --- | --- | --- | --- | --- | --- |
| V1 | High | Test file of lot F4 (`tests/unit/changeset-links.test.ts`) | line 17 | `changesetIdsOf([{ changesetId: null }, {}])`: `{}` has no `changesetId`, so `npm run test:types` fails (TS2741). Runtime is not affected. | Use `{ changesetId: null }` (the reducer always sets the field). |
| V2 | Medium | Lot 3, history and conversation (`src/lib/features/changes/ConversationChanges.svelte`) | 75 (empty title « Aucune modification » and its description), 82-83 (permanent « Chiffres : somme des opérations d’édition (pas un diff Git net) »), 103 (`countLabel`) | Spec §6 removes these from « Modifications → Historique ». The Conversation tab of the same panel still shows them, which is the same kind of permanent text in a zone the lot touched. | Remove the empty-state branch that shows a title, keep « Chargement… » and « Réessayer »; delete the « Chiffres » paragraph and the `countLabel` prop. Then change the `it.fails` in `leftovers-14-3.test.ts` to `it`. |
| V3 | Low | Lot 2, skills (`src/lib/features/settings/SkillsSettings.svelte`) | 117-119 | Permanent paragraph « Consulter cette liste n’approuve aucun skill de projet… ». Spec §9 asks for no permanent operating paragraphs. The fact is still useful. | Move it into the project source label or a tooltip, or keep it if the orchestrator accepts it as a safety statement. |
| V4 | Low | Lot 2, models (`src/lib/features/settings/ModelFormDialog.svelte`) | 264-266 | Permanent empty-state text « Aucun profil : le composeur n’offre pas de choix d’effort… ». Spec §6: no repeated empty states. | Remove it; the chip group already shows the empty set. |
| V5 | Low | Lot 2, appearance (`src/lib/features/settings/AppearanceSettings.svelte`) | 88 | Helper line « Valeurs du mode clair/sombre en cours. ». Spec §5: no new explanation rows. | Drop the `description` prop, or fold the mode name into the group title. |
| V6 | Low | Lot 3, header (`src/lib/features/conversation/ConversationTitle.svelte`) | 60 (comment), 76 and 87 (two rename controls) | The comment says « The page's only h1 », but the element is an `h2`; the page h1 is the sr-only « Bricks » (`AppShell.svelte:301`). Two controls do the same rename (the title button and « Modifier le titre »). | Correct the comment. Keep one rename control, or keep both if the orchestrator wants the visible affordance. |
| V7 | Low | Shared UI (`src/lib/components/ui/sidebar/sidebar-trigger.svelte`) | 20 | Icon-only button with no accessible name. Nothing imports this component (grep). | Add an `aria-label`, or delete the unused component. |

Checked and not defects: the header is three columns (`TopBar.svelte`: `grid-cols-[minmax(0,1fr)_minmax(0,min(28rem,50%))_minmax(0,1fr)]`), the left toggle is in the first column, the
send tooltip reads « Envoyer », the placeholder is « Quelque chose à dire ? », the composer textarea and the
user bubble have `dir="auto"` and `text-start`, `{@html}` appears only in `AssistantMessage.svelte` (sanitised by DOMPurify in `markdown.ts`),
and no Inter or fontsource reference remains in `src/` or the lockfile. Every icon-only button under
`src/lib/` has a name, except V7 (scan of `Button` and `button` tags).

## 5. Notes (no action required)

- N1. Model form: `buildModelPatch` sends `context_window_tokens: null` and `clear_context_window: false`, and the Rust side keeps the stored window. Confirmed by `profiles_14_3.rs` and `providers_toml.rs`.
- N2. The ColorPicker keyboard path is the two sliders and the hex field. The saturation area is pointer-only and says so in a comment. The autofixer raises no issue.
- N3. The composer's skill refusal: `Command::Submit` checks the skills in `controller.rs` before anything runs and rejects otherwise. `Composer.svelte` keeps the draft, attachments and chips when `lastRejection` is a submit from after the send. The `pending.skill` branch only applies to `/skill`. Reading only; not a unit test (no pure function).
- N4. Links in the assistant Markdown renderer use the raw `text` of marked tokens, so emphasis inside link text shows its markers. This code is unchanged in 14.3 (`git diff` shows only the import move), so it is not a 14.3 defect.
- N5. `bench/term-bench/runner-google 2.sh` remains (ignored by `.gitignore`, flagged in `docs/cleanup-14-3.md` §6). Not touched.
- N6. Not reviewed in depth: `ChangesetView`, `AgentInspector` changeset section, `JobsView` after reopen (known gap in `docs/adk-matrix-14-3.md`), `GitChanges` (its « Aucune modification : l’arbre de travail est propre. » line is the Git tab, outside spec §6).

## 6. Files written by this validation

- `tests/unit/render-14-3.test.ts` (10 tests, passing)
- `tests/unit/appearance-14-3.test.ts` (5, passing)
- `tests/unit/leftovers-14-3.test.ts` (11 passing, 1 `it.fails` for V2)
- `src-tauri/tests/profiles_14_3.rs` (10, passing)
- `src-tauri/tests/skills_prompt_14_3.rs` (4, passing)
- `tests/REVIEW_14_3.md` (this file), and the « Sprint 14.3 » sections of `tests/README.md`

No product file was edited. No file under `~/.bricks` was read or written; the Rust tests use `tempfile`.
