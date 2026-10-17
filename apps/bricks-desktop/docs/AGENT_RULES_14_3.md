# Sprint 14.3 — additional rules (read AGENT_RULES.md, _14_1.md, _14_2.md first; they still apply)

Spec: `docs/sprint-14-3-ui.md` (repo root) — read the sections of your lot. Base `e62a6be`.

> Implémente le lot attribué avec le moteur et les contrats existants. N’ajoute aucune nouvelle capacité
> métier. Ne supprime aucune API publique. N’invente ni données, ni résultat de test, ni validation
> native. Signale immédiatement un contrat manquant ou une dépendance à un autre lot. Livre le code
> intégré, les contrôles réellement exécutés et les défauts encore ouverts. Ne déclare pas terminé un
> parcours dont seul le bouton ou le type existe.

## Contracts fixed by the orchestrator (read, don't change — report needs)
- `Preferences.colors: ThemeColors { light: ColorSet, dark: ColorSet }`, `ColorSet { accent, background,
  panel }` (`#rrggbb` or null = theme token). `Preferences.terminal_ratio: number | null` (0.1–0.8 of the
  centre column; null → derived once from legacy `terminal_height`). Saved with `app.setPrefs(...)`.
- `ModelProfileView { id, label, parameters, remove }`; `ModelPatch.profiles: ReasoningProfileEdit[] | null`
  (replaces the whole list; `ReasoningProfileEdit { id, label, parameters: object, remove: string[] }`).
- `skillsCatalog(projectId | null, query, page, reload)` → `SkillsCatalog { project_id, listing
  (engine SkillsListing), sources }` — no trust granted.
- `conversation.draft.skills: string[]` (composer) → `Prompt.skills` at send.
- Fonts vendored in `src/assets/fonts/geist/` (Geist-Variable, Geist-Italic-Variable, GeistMono-Variable
  woff2 + OFL.txt). Inter (`@fontsource-variable/inter`) is to be dropped by the appearance lot.
- Orchestrator-owned (never edit): `src-tauri/src/contract.rs`, `src-tauri/src/lib.rs`, `src/lib/ipc/*`,
  `src/lib/state/*`, `package.json`, lockfiles, any `Cargo.toml`. Shared files have ONE owner (listed in
  your brief).
- Never touch `crates/cersei-agent/src/system_prompt.rs` (user's uncommitted change), `bricks.toml`,
  `docs/sprint-14*.md`. Do not reintroduce the TUI.
