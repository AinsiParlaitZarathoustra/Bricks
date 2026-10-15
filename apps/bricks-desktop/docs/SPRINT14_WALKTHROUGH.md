# Sprint 14 — Bricks Desktop : walkthrough

Branche `Bricks_Desktop` (base `66d5076`). Rien n’est commité ni poussé.

## Commandes

```bash
cd apps/bricks-desktop
npm install
npm run tauri dev -- --no-watch   # développement (--no-watch : le dossier Documents est synchronisé et déclenche des rechargements)
npm run check                     # svelte-check + TypeScript
npm run test:unit                 # Vitest (réducteur de conversation, formats)
npx tauri build --debug --bundles app   # bundle macOS : target/debug/bundle/macos/Bricks.app
cargo test -p bricks-desktop -p bricks-config
cargo test -p bricks-cli --bin bricks --test headless --test system_prompt --test workspace
```

Types TypeScript du contrat : `cargo test -p bricks-desktop services::export::export_bindings -- --exact`
(écrit `src/lib/ipc/bindings/`).

## Architecture

```text
Composants Svelte (src/lib/features/*, src/lib/layout)
  → état global (src/lib/state : app + Conversation par session) et client IPC typé (src/lib/ipc)
    → commandes Tauri + un Channel d’événements multiplexé (src-tauri/src/lib.rs, contract.rs)
      → services desktop (engine/, services/, commands/, git/, pty/, paths.rs)
        → Controller / Command / Envelope du moteur (cersei-agent::control)
```

- **Contrat** : `src-tauri/src/contract.rs` (types ts-rs → `src/lib/ipc/bindings/`) ; types JSON du moteur
  reflétés dans `src/lib/ipc/engine.ts` (schéma 5).
- **Assemblage moteur partagé** : nouvelle crate `crates/bricks-config` (home Bricks, registre de
  fournisseurs, mémoire long terme, `ProjectLoader`, `engine_config`) utilisée par la CLI et le desktop ;
  `bricks-cli/src/setup.rs` n’est plus qu’un adaptateur des arguments.
- **Registre de contrôleurs** : un `Controller` par session ouverte, flux consommé une fois par une tâche
  qui diffuse aux abonnés (fan-out borné, événement `lagged` si un abonné décroche), tampon des 4000
  derniers envelopes pour la reprise d’affichage ; une session ne peut pas avoir deux propriétaires.
- **Persistance ajoutée** (`~/.bricks/desktop/`) : `projects.json`, `preferences.json`, journal des
  modifications `changes/<projet>.jsonl`, pièces jointes temporaires (20 Mo max, purge à 7 jours).
- **Moteur** : seul ajout, `Controller::set_title` (un renommage survit aux sauvegardes d’un run).

## Contributions des agents

Tous lancés par l’orchestrateur avec `model: haiku` et `effort: xhigh`. Modèle rapporté par chaque agent :
`claude-haiku-5-5`. L’effort n’est pas visible dans le prompt système des agents : il est appliqué par le
lanceur mais n’a pas pu être confirmé depuis l’intérieur des agents.

| Agent | Livré | Vérifications rapportées |
| --- | --- | --- |
| Haiku 1 — Intégration Rust | `bricks-config`, `engine/`, `services/`, `commands/`, bindings ts-rs, `set_title` | 69 tests desktop, tests CLI ciblés, build CLI/TUI |
| Haiku 2 — Design system | tokens clair/sombre/système (AA), `AppShell` 3 zones + terminal, `TopBar`, primitives | MCP Svelte, contrastes calculés, contrôle visuel hors Tauri |
| Haiku 3 — Navigation | projets, conversations, renommage, suppression confirmée, recherche titres + messages | MCP Svelte, `npm run check` |
| Haiku 4 — Conversation | timeline Markdown sûre, outils, approbations, modifications, résumé final réel, liens externes | MCP Svelte, tests Node du parseur de tests |
| Haiku 5 — Composeur | branche Git réelle, brouillon par conversation, pièces jointes (choix, collage, dépôt), modèle/effort du catalogue, annulation | MCP Svelte, `npm run check` |
| Haiku 6 — Contexte et agents | `ContextRing.svelte` (provenance fiable seulement), environnement, sous-agents, inspecteur, ChangeSets avec confirmation | MCP Svelte, `npm run check` |
| Haiku 7 — Git et terminal | Rust : statut porcelain v2, diffs, log/show, PTY réel ; Svelte : 3 vues de modifications, xterm.js | 34 tests Rust, MCP Svelte, build Vite |
| Haiku 8 — Paramètres et qualité | page Paramètres, stratégie de tests, specs WebDriver, tests unitaires, revue a11y (17 points, corrigés par les propriétaires) | 19/19 tests Vitest |

Orchestrateur : socle Tauri/Vite/Tailwind/shadcn, contrat IPC, `engine.ts`, client IPC, état global,
`App.svelte`, dépendances, raccordement CLI, correctifs d’intégration ciblés.

## MCP Svelte

Chaque agent frontend a appelé `list-sections`, `get-documentation` et `svelte-autofixer` sur ses
composants (détail dans leurs rapports). Les suggestions restantes concernent des `$effect` volontaires
(E/S, minuteries, synchronisation impérative) ; le seul `{@html}` passe par DOMPurify.

## Validation effectuée

- `npm run check` : 0 erreur (1 avertissement : `baseUrl` déprécié dans `tsconfig.app.json`).
- `npm run test:unit` : 19/19.
- Rust : build `bricks-desktop`, `bricks-cli`, `bricks-tui` ; tests desktop (69 + 34 git/pty), `bricks-config`,
  tests CLI ciblés (headless 8, workspace 5, system_prompt 1, bin 5–6).
- macOS : `Bricks.app` (debug) se construit et s’ouvre (fenêtre « Bricks »).

## Non validé — le sprint reste ouvert sur ces points

- **Parcours dans l’application démarrée** (projet réel, streaming fournisseur, approbation, annulation,
  pièces jointes, sous-agents simultanés, terminal, redimensionnements, thèmes, redémarrage) : non exécutés.
  La capture d’écran automatisée a échoué sur cette machine (erreur de capture audio/vidéo), elle a fait
  planter une session. Checklist manuelle : `tests/README.md`.
- **Windows / Linux** : non compilés ni testés. Specs WebDriver prêtes (`tests/e2e/`), dépendances
  `@wdio/*` non installées.
- **Limites connues** : patchs du journal seulement pour les éditions passées par un aperçu
  d’approbation ; effets des shells/MCP non attribués (indiqué dans l’UI) ; recherche de conversations
  sans index (100 résultats) ; course moteur préexistante Cancel juste après Submit (contournée côté
  desktop par renvoi de l’annulation) ; modèle/effort par défaut et réglages avancés « À venir ».
- Les fichiers non suivis `crates/bricks-cli/tests/* 2.rs` (préexistants) cassent `cargo test -p bricks-cli`
  global : non touchés.
