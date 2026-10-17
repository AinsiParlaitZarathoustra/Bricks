# Sprint 14.3 — Copies locales « … 2 » : manifeste de nettoyage

Périmètre : fichiers non suivis (`git ls-files --others --exclude-standard`) dont le nom contient ` 2` (`X 2.ext`, `X 2`, ou composant de dossier `X 2`). Exclus : `.git/`, `target/`, `node_modules/`. Référence : `docs/sprint-14-3-ui.md` §3 « Copies locales et résidus TUI », règles `AGENT_RULES_14_3.md`.

## 1. Résultat

| Catégorie | Nombre | Traitement |
|---|---:|---|
| Identique à l'original présent (octets égaux) | 633 | supprimée ; l'original reste intact |
| Différente de l'original, contenu retrouvé dans l'historique Git | 20 | supprimée ; récupérable au commit indiqué §3 |
| Sans original présent, contenu retrouvé dans l'historique Git | 24 | supprimée ; récupérable au commit indiqué §4 |
| Différente ou sans original, contenu **non** retrouvé | 0 | sauvegardée puis supprimée (voir §2) |
| **Total** | **677** | |

- Dossiers nommés `… 2` : 0 (aucun contenu à classer par dossier).
- Liens symboliques parmi les copies : 0.
- Dossiers vides supprimés après retrait des copies : 3 (`crates/bricks-tui/src/`, `crates/bricks-tui/tests/`, `crates/bricks-tui/`).
- `crates/bricks-tui/` n'était plus suivi par Git ni membre du workspace (`Cargo.toml` racine) ; tout son contenu était constitué de copies.

## 2. Copies conservées hors du workspace

Aucune. Chaque copie « différente » ou « sans original » a son contenu exact dans l'historique Git ; aucun dossier `~/bricks-sync-duplicates-backup/` n'a donc été créé.

## 3. Copies « différentes » : commits de récupération

20 copies. Le commit indiqué est le plus récent (parmi ceux qui touchent ce contenu) dont l'arbre contient exactement ce blob au chemin indiqué : `git show <commit>:<chemin>` restitue la version.

| Copie supprimée | Original actuel | Contenu récupéré à |
|---|---|---|
| `Cargo 2.lock` | `Cargo.lock` | `cbbc593:Cargo.lock` |
| `Cargo 2.toml` | `Cargo.toml` | `cbbc593:Cargo.toml` |
| `README 2.md` | `README.md` | `cbbc593:README.md` |
| `crates/bricks-cli/Cargo 2.toml` | `crates/bricks-cli/Cargo.toml` | `54079af:crates/bricks-cli/Cargo.toml` |
| `crates/bricks-cli/src/args 2.rs` | `crates/bricks-cli/src/args.rs` | `d7d5c69:crates/bricks-cli/src/args.rs` |
| `crates/bricks-cli/src/main 2.rs` | `crates/bricks-cli/src/main.rs` | `fa09100:crates/bricks-cli/src/main.rs` |
| `crates/bricks-cli/src/setup 2.rs` | `crates/bricks-cli/src/setup.rs` | `66d5076:crates/bricks-cli/src/setup.rs` |
| `crates/bricks-cli/tests/workspace 2.rs` | `crates/bricks-cli/tests/workspace.rs` | `47398f7:crates/bricks-cli/tests/workspace.rs` |
| `crates/cersei-agent/src/agents/spawn 2.rs` | `crates/cersei-agent/src/agents/spawn.rs` | `66d5076:crates/cersei-agent/src/agents/spawn.rs` |
| `crates/cersei-agent/src/control/approval 2.rs` | `crates/cersei-agent/src/control/approval.rs` | `fa09100:crates/cersei-agent/src/control/approval.rs` |
| `crates/cersei-agent/src/control/controller 2.rs` | `crates/cersei-agent/src/control/controller.rs` | `cbbc593:crates/cersei-agent/src/control/controller.rs` |
| `crates/cersei-agent/src/control/mod 2.rs` | `crates/cersei-agent/src/control/mod.rs` | `47398f7:crates/cersei-agent/src/control/mod.rs` |
| `crates/cersei-agent/src/control/settings 2.rs` | `crates/cersei-agent/src/control/settings.rs` | `47398f7:crates/cersei-agent/src/control/settings.rs` |
| `docs/cli 2.md` | `docs/cli.md` | `cbbc593:docs/cli.md` |
| `docs/content/docs/architecture 2.mdx` | `docs/content/docs/architecture.mdx` | `d7d5c69:docs/content/docs/architecture.mdx` |
| `docs/content/docs/crate-map 2.mdx` | `docs/content/docs/crate-map.mdx` | `cbbc593:docs/content/docs/crate-map.mdx` |
| `docs/content/docs/sessions 2.mdx` | `docs/content/docs/sessions.mdx` | `54079af:docs/content/docs/sessions.mdx` |
| `docs/maintenance 2.md` | `docs/maintenance.md` | `d7d5c69:docs/maintenance.md` |
| `sync_cargo 2.sh` | `sync_cargo.sh` | `d7d5c69:sync_cargo.sh` |
| `wiki/09-architecture 2.md` | `wiki/09-architecture.md` | `cbbc593:wiki/09-architecture.md` |

## 4. Copies « sans original » : commits de récupération

24 copies, dont tout `crates/bricks-tui/` (TUI retirée en Sprint 14.1). Les versions restent accessibles dans l'historique ; l'arbre courant n'en contient plus d'original.

| Copie supprimée | Original actuel | Contenu récupéré à |
|---|---|---|
| `crates/bricks-cli/src/tui 2.rs` | `crates/bricks-cli/src/tui.rs` | `fa09100:crates/bricks-cli/src/tui.rs` |
| `crates/bricks-cli/tests/pty_tui 2.rs` | `crates/bricks-cli/tests/pty_tui.rs` | `47398f7:crates/bricks-cli/tests/pty_tui.rs` |
| `crates/bricks-tui/Cargo 2.toml` | `crates/bricks-tui/Cargo.toml` | `fa09100:crates/bricks-tui/Cargo.toml` |
| `crates/bricks-tui/src/commands 2.rs` | `crates/bricks-tui/src/commands.rs` | `47398f7:crates/bricks-tui/src/commands.rs` |
| `crates/bricks-tui/src/composer 2.rs` | `crates/bricks-tui/src/composer.rs` | `d7d5c69:crates/bricks-tui/src/composer.rs` |
| `crates/bricks-tui/src/diag 2.rs` | `crates/bricks-tui/src/diag.rs` | `47398f7:crates/bricks-tui/src/diag.rs` |
| `crates/bricks-tui/src/inspect 2.rs` | `crates/bricks-tui/src/inspect.rs` | `d7d5c69:crates/bricks-tui/src/inspect.rs` |
| `crates/bricks-tui/src/lib 2.rs` | `crates/bricks-tui/src/lib.rs` | `cbbc593:crates/bricks-tui/src/lib.rs` |
| `crates/bricks-tui/src/links 2.rs` | `crates/bricks-tui/src/links.rs` | `47398f7:crates/bricks-tui/src/links.rs` |
| `crates/bricks-tui/src/markdown 2.rs` | `crates/bricks-tui/src/markdown.rs` | `cbbc593:crates/bricks-tui/src/markdown.rs` |
| `crates/bricks-tui/src/mentions 2.rs` | `crates/bricks-tui/src/mentions.rs` | `fa7af2b:crates/bricks-tui/src/mentions.rs` |
| `crates/bricks-tui/src/overlay 2.rs` | `crates/bricks-tui/src/overlay.rs` | `47398f7:crates/bricks-tui/src/overlay.rs` |
| `crates/bricks-tui/src/render_cache 2.rs` | `crates/bricks-tui/src/render_cache.rs` | `cbbc593:crates/bricks-tui/src/render_cache.rs` |
| `crates/bricks-tui/src/state 2.rs` | `crates/bricks-tui/src/state.rs` | `cbbc593:crates/bricks-tui/src/state.rs` |
| `crates/bricks-tui/src/statusbar 2.rs` | `crates/bricks-tui/src/statusbar.rs` | `cbbc593:crates/bricks-tui/src/statusbar.rs` |
| `crates/bricks-tui/src/term 2.rs` | `crates/bricks-tui/src/term.rs` | `47398f7:crates/bricks-tui/src/term.rs` |
| `crates/bricks-tui/src/ui 2.rs` | `crates/bricks-tui/src/ui.rs` | `cbbc593:crates/bricks-tui/src/ui.rs` |
| `crates/bricks-tui/src/view 2.rs` | `crates/bricks-tui/src/view.rs` | `cbbc593:crates/bricks-tui/src/view.rs` |
| `crates/bricks-tui/tests/interaction 2.rs` | `crates/bricks-tui/tests/interaction.rs` | `cbbc593:crates/bricks-tui/tests/interaction.rs` |
| `crates/bricks-tui/tests/links 2.rs` | `crates/bricks-tui/tests/links.rs` | `47398f7:crates/bricks-tui/tests/links.rs` |
| `crates/bricks-tui/tests/render 2.rs` | `crates/bricks-tui/tests/render.rs` | `cbbc593:crates/bricks-tui/tests/render.rs` |
| `crates/bricks-tui/tests/status_bar 2.rs` | `crates/bricks-tui/tests/status_bar.rs` | `cbbc593:crates/bricks-tui/tests/status_bar.rs` |
| `crates/bricks-tui/tests/totals 2.rs` | `crates/bricks-tui/tests/totals.rs` | `47398f7:crates/bricks-tui/tests/totals.rs` |
| `crates/bricks-tui/tests/workspace 2.rs` | `crates/bricks-tui/tests/workspace.rs` | `47398f7:crates/bricks-tui/tests/workspace.rs` |

## 5. Commits cités

| Commit | Date | Sujet |
|---|---|---|
| `fa7af2b` | 2026-10-06 | Bricks release |
| `fa09100` | 2026-10-08 | Fix resumed sessions' project context + Add workspaces, history by project, terminal links |
| `54079af` | 2026-10-10 | Technical cleanup: dependencies, SSE bound, Bash analysis, repository hygiene (0.5.0) |
| `d7d5c69` | 2026-10-11 | Agent Skills, /effort, readable cost and context, adaptive status bar (0.6.0) |
| `47398f7` | 2026-10-11 | Move to Rust edition 2024 (0.6.0) |
| `66d5076` | 2026-10-14 | Changement du prompt système (0.6.1) |
| `cbbc593` | 2026-10-14 | Responsiveness, cancellation and reliable session saves (0.6.1) |

## 6. Méthode

1. Pour chaque copie `X 2.ext`, l'original est `X.ext` (même dossier). Un composant de dossier `X 2` correspondrait à `X`.
2. Original présent et fichier identique (comparaison octet à octet) : catégorie « identique ».
3. Sinon : empreinte du blob `git hash-object --no-filters` ; présence du blob dans `git rev-list --objects --all` ; contrôle que les octets du blob (`git cat-file blob`) ont le même SHA-256 que la copie ; commit de récupération par `git log --all --find-object=<blob>` puis `git ls-tree -r <commit>`.
4. Blob absent de l'historique : copie sauvegardée hors du workspace (SHA-256 vérifié), puis supprimée du workspace.
5. Suppressions par chemin explicite issu du manifeste (aucun `rm -rf` ni `git clean`), puis `rmdir` des seuls dossiers devenus vides.
6. Les originaux ne sont jamais écrits.

Hors périmètre, non touché :
- `bench/term-bench/runner-google 2.sh` : ignoré par `.gitignore` (`bench/**/runner-*.sh`), donc pas « non suivi ». Identique à `bench/term-bench/runner-google.sh` (`cmp`). À supprimer si souhaité.
- `.git/objects/*` (entrées `… 2` signalées par `git count-objects`) : interne à Git, non modifié.

## 7. Vérifications

- `cargo metadata --format-version 1 --no-deps > /dev/null` : rc=0.
- Cibles Cargo (25 paquets) dont le chemin source contient ` 2` : 0.
- `cargo build --workspace` : 2e passage rc=0, 0 avertissement. Un 1er passage a échoué (14 min) avec 3 erreurs `unresolved crate bricks_skills` dans `apps/bricks-desktop/src-tauri/src/services/skills.rs`, fichier non suivi, modifié en parallèle par un autre agent ; il n'est plus déclaré dans le module tree au 2e passage. Aucun lien avec les copies supprimées (aucune copie ne visait ce chemin, `apps/bricks-desktop/src-tauri/Cargo.toml` non modifié).
- `git status --porcelain --untracked-files=all | grep ' 2'` : 0 ligne.
- Recherche `* 2*` hors `.git/`, `target/`, `node_modules/` : 1 résultat, `bench/term-bench/runner-google 2.sh` (ignoré, hors périmètre).
- Fichiers suivis par Git : 1115 avant et après (aucun fichier suivi touché).
