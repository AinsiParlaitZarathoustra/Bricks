# Review, Sprint 14.2 (validation)

Scope: the Svelte files touched in 14.2 (context, tool lines, approval card, settings navigation,
settings dialog and models page, composer entry points) and the Rust approval override, providers
editing and preferences. Read-only: no feature file was edited. Line numbers are as of 2026-10-16.

Svelte MCP: `svelte-autofixer` (read-only, Svelte 5) on `ApprovalCard.svelte`, `ToolLine.svelte`,
`ModelSettings.svelte`, `ModelRow.svelte`, `GeneralSettings.svelte`, `ProviderFormDialog.svelte`:
no issues, no suggestions.

Severity: **medium** = wrong state or data risk in a normal flow; **low** = usability, a11y detail or
cost; **info** = noted, no action required for 14.2.

## Medium

**M1. Approval preference shown before it is saved; no rollback on failure.**
`src/lib/state/app.svelte.ts:130-136`. `setPrefs` writes `this.prefs` at once and sends `prefs_set`
after a 300 ms debounce; a failure only calls `fail(e)` (line 134). `GeneralSettings.svelte:21`
then shows the new policy while the engine still runs the old one. The debounce also means a change
reaches the engine about 300 ms later, and is lost if the app quits inside that window.
Fix: on failure, reload the saved preferences (or restore the last saved snapshot); flush the pending
save before quit. Owner: orchestrator (`app.svelte.ts`).

**M2. Settings can be unreachable.**
`src/lib/features/navigation/NavigationPanel.svelte:102-112`: the only call that opens Settings is
`ui.settingsOpen = true` (line 107), in the footer of the left panel. With a wide window and the left
panel collapsed (`left_open` false), nothing opens Settings. Fix: add an entry point that does not
depend on the left panel (top bar item or a palette command). Owner: orchestrator (AppShell and
TopBar; the top-bar button was removed on purpose, so this needs a decision).

**M3. A conflict can lose a concurrent change without the user seeing it.**
`src/lib/features/settings/ProviderFormDialog.svelte:38,53` and
`ModelFormDialog.svelte:40,62`: `original` is captured once at mount (`untrack`). After a `conflict`,
`ModelSettings.svelte:73` reloads the file, but the dialog keeps the old `original`. The message says
to check the changes and retry, yet the retry builds the patch from the old values, so a field that
someone else changed is overwritten silently. Fix: on conflict, close the dialog, or rebase the form on
the reloaded provider or model and show the new values. Owner: settings (models page).

**M4. An "allow for this session" given under « Toujours demander » is still recorded.**
`crates/cersei-agent/src/control/approval.rs:207` inserts the grant whatever the mode. The grant is
ignored while the mode is `always_ask`, but it applies again once the mode returns to `standard` in the
same session. The card still offers « Autoriser pour la session » (`ApprovalCard.svelte:126-127`) in
that mode, where it has no effect. Decision needed: do not record grants under `AlwaysAsk`, or hide or
relabel the button in that mode. Not pinned by the new tests on purpose. Owner: cersei-agent (contract
decision) and approval card.

## Low

**L1. An unknown approval value discards every preference.**
`src-tauri/src/services/prefs.rs:31-33`: an unparseable file (for example `tool_approval: "yolo"`)
falls back to all defaults, so theme, widths and last project are lost, and the next save overwrites
the file. The policy itself is safe: `tests/preferences_approval.rs` checks that an unknown value never
gives `no_confirmation`. Fix: parse `tool_approval` per field, unknown → `standard`, keep the rest.
Owner: services (preferences).

**L2. `aria-controls` points to a missing element while collapsed.**
`src/lib/features/settings/ModelRow.svelte:30`: the detail block exists only when expanded (line 73).
Fix: `aria-controls={expanded ? detailId : undefined}`.

**L3. No reload action for an invalid providers file.**
`src/lib/features/settings/ModelSettings.svelte:134-139`: the load-error state has « Réessayer »
(line 121), this state does not. After fixing the file the user has to close and reopen Settings.
Fix: add `<Button variant="outline" size="sm" onclick={load}>` in that block.

**L4. A model with no declared modality is shown as « Texte ».**
`src/lib/features/settings/format.ts:28`: `formatModalities([])` returns `'Texte'`, which states a
capability the file does not declare. Fix: return « Non déclaré ».

**L5. An empty effort-profile label renders as blank.**
`src/lib/features/settings/ModelRow.svelte:101` and `ModelFormDialog.svelte:199-200`: `label ?? id`
keeps an empty string. Fix: use `||`.

**L6. Icon barrel imports slow the tests.**
`ToolLine.svelte:4`, `ApprovalCard.svelte:2`, `ProviderGroup.svelte:3`, `ModelRow.svelte:3` import from
`@lucide/svelte`; `SettingsDialog.svelte:7-11` uses deep imports (`@lucide/svelte/icons/<name>`).
With the barrel, the server render in `tests/unit/toolline-render.test.ts` takes about 15 s of
transform time. Fix: deep imports. Owner: feature owners. No effect on the production build.

**L7. The context card still holds the billed totals, under the occupation line.**
`src/lib/features/context/ContextCard.svelte:19-22`: the occupation line is correct (`≈ N / M`), but the
card goes on to show « Consommation cumulée (facturée) ». The brief said the cumulative sentence was
removed; the block remains, labelled. Check that this is the intended layout.

## Info

- **I1.** The manual checklist (`tests/README.md`, item « Reject ») said « Refusé (par vous) ». The
  product shows « Refusé · Write · par vous » (`toolLine.ts`, `approvalTrace`). The checklist is updated
  in this sprint.
- **I2.** `ModelSettings.svelte:193-202` shows « À venir » for the two default-model rows. There are
  no buttons; the state is honest.
- **I3.** `ApprovalCard.svelte:33-46`: `sending` is kept until `approval_resolved` arrives, so the
  buttons stay disabled if that event never comes. This is the designed behaviour; a refusal is shown.
- **I4.** `NavigationPanel.svelte`, `Composer.svelte:121,850,958,971,137`: the « Quelque chose à dire ? »
  placeholder, the `/` palette and the `@` mention palette are all wired. `npm run check`: 0 errors.
- **I5.** Markdown in `AssistantMessage.svelte:48` is sanitised by DOMPurify in `markdown.ts:86-89`.
- **I6.** Build infrastructure, not product: the files `crates/cersei-agent/tests/* 2.rs` (user data,
  not touched) are auto-discovered as test targets, and cargo rejects their names
  (`invalid character ' ' in crate name`). So `cargo test -p cersei-agent` without `--lib` or `--test`
  does not build. Fix: owner's decision (rename or remove the copies, or `autotests = false` in
  `crates/cersei-agent/Cargo.toml`).
