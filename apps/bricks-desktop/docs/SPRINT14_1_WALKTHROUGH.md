# Sprint 14.1 — Simplification et migration de la TUI : walkthrough

Branche `Bricks_Desktop`, à partir de `9e49beb` (Sprint 14). Rien n’est commité ni poussé.
Spécification : `docs/sprint-14-1-ui.md`. Inventaire détaillé de la TUI avant retrait :
`apps/bricks-desktop/docs/tui-inventory.md`.

## 1. Résultat

- **Composeur** : placeholder exact `Quelque chose à dire ?`. L’aide Entrée / Maj+Entrée est passée dans
  l’infobulle du bouton d’envoi et dans l’aide `/help`. Les messages des états bloqués et le message
  pendant une exécution sont conservés.
- **Paramètres** : la rubrique « Interface » devient « Général » (`GeneralSettings.svelte`, id `general`).
  Retirés : « Panneaux à l’ouverture » (3 interrupteurs), « Dimensions » (px), « Réinitialiser la
  disposition ». Une seule ligne factuelle : la disposition est mémorisée automatiquement. Les
  séparateurs, boutons de panneaux et la mémorisation automatique sont inchangés.
- **TUI** : toutes ses fonctions métier passent par un registre unique partagé
  (`cersei_agent::control::slash`), exposé au desktop par IPC, avec une palette `/` dans le composeur et
  des vues d’inspection. La crate `bricks-tui` est ensuite retirée.

## 2. Matrice de reprise

Accès desktop : `/` = palette du composeur (registre partagé `slash_commands` / `slash_parse`).
Vérification : **R** = `src-tauri/tests/slash_mapping.rs` (33 noms et alias → action),
**D** = `tests/unit/dispatch-coverage.test.ts` (chaque action a un effet desktop, aucune ligne `/`
envoyée comme prompt sauf `/skill`), **U** = tests unitaires du réducteur / de la vue.
**N** = parcours dans l’application démarrée : *non vérifié* (voir §6).

| Fonction TUI | Accès desktop | Commande / API réelle | Vérif. |
| --- | --- | --- | --- |
| `/model`, `/m` [modèle [profil]] | `/` → sélecteur du composeur ou changement direct | `set_model` | R D N |
| `/effort` [profil \| default] | `/` → sélecteur d’effort | `set_reasoning` (`default` → `profile: null`) | R D N |
| `/search`, `/s`, `re:` | vue « Recherche dans le code » | `search` → `search_results` | R D U N |
| `/agents` [mots \| reload] | vue Profils d’agents | `list_agent_profiles`, `reload_agent_profiles` → `agent_profiles` | R D U N |
| `/agents running\|status\|result\|cancel` | vue Sous-agents (AgentList / AgentInspector) | `agent_control` list/status/result/cancel | R D N |
| `/skills` [mots, page \| reload \| trust/allow \| deny/refuse] | vue Skills | `list_skills`, `reload_skills`, `trust_project_skills` → `skills` | R D U N |
| `/skill <nom> <demande>` | `/` | `submit` avec `prompt.skills` ; refus moteur → rien n’est lancé, brouillon conservé | R D U N |
| `/changes <id> inspect\|apply\|discard` | vue ChangeSet (confirmation, conflits) | `agent_control` inspect/apply/discard_changes | R D N |
| `/jobs`, `/jobs stop <id>` | vue Tâches de fond (≠ terminaux PTY) | `agent_control` jobs / stop_job ; `job_*` | R D U N |
| `/memory` | vue Mémoire | snapshot `memory_space`, `memory_recalled`, `memory_maintenance_*` | R D U N |
| `/context`, `/ctx` [details] | vue Contexte (ContextRing + détails) | `Snapshot.context`, événements `context` | R D N |
| `/cost`, `/usage` [details] | vue Consommation | `usage` (par réponse), `run_usage`, snapshot `usage` ; prix inconnu = inconnu | R D U N |
| `/session` | vue Session | `session_snapshot` | R D N |
| `/resume`, `/sessions` [id] | vue Reprise / `app.resumeSession` | `session_locate` + `session_open` (registre desktop, jamais `resume` moteur) | R D N |
| `/compact` | `/` (progression/résultat près du composeur) | `compact` → `compaction` | R D U N |
| `/clear` | vue Vider le contexte (explication + confirmation) | `clear_context` → `context_cleared` | R D U N |
| `/diff` | panneau droit « Modifications » | journal, ChangeSets, Git (Sprint 14) | R D N |
| `/tools` | vue Outils | `session_tools` (`Controller::tools()` + règles) | R D N |
| `/mcp` | vue Serveurs MCP | `session_mcp` (`Controller::mcp_statuses()`) | R D N |
| `/config` | vue Configuration (sans secret) | `session_config` | R D N |
| `/file`, `/folder`, `/image`, `/img` | pièces jointes du brouillon | `PromptBlock` file/folder/image, `path_kind` | R D N |
| `/help`, `/?` | vue Commandes (générée depuis le registre) | `slash_commands` | R D N |
| `/quit`, `/exit`, `/q` | confirmation si exécutions actives, puis fermeture sûre | `app_quit` (engine + PTY shutdown, idempotent) | R D N |
| `@` mentions (index de fichiers) | popover `@` du composeur, Ctrl+R / Actualiser | `project_files` (gitignore, nucleo) ; invalidé sur `edit_applied` | U N |
| Historique des saisies (↑/↓) | composeur, brouillon courant préservé | local (prompts de la conversation) | U N |
| Annulation, approbations avec aperçu, détails d’outils, reasoning repliable, défilement de l’historique | déjà présents (Sprint 14) | `cancel`, `approve`, timeline | N |

## 3. Éléments TUI retirés

- Crate `crates/bricks-tui` (22 fichiers suivis), son membre de workspace et sa dépendance workspace.
- Dépendances workspace devenues inutiles : `ratatui`, `pulldown-cmark`, `unicode-segmentation`,
  `unicode-width` (`ignore`, `nucleo-matcher`, `url`, `portable-pty` conservées : encore utilisées).
  `Cargo.lock` : 73 paquets retirés, aucun ajouté, aucune version modifiée.
- `bricks-cli` : `src/tui.rs`, `tests/pty_tui.rs`, sous-commandes `tui` et `resume`, option `--hyperlinks`.
  `bricks` sans sous-commande affiche l’aide (code 0). Conservés à l’identique : `bricks run` (JSONL,
  `--session`, pièces jointes, skills, permissions) et `bricks sessions`. Reprise headless :
  `bricks run --session <id> …`.
- Documentation active mise à jour : `README.md`, `docs/cli.md`, `docs/maintenance.md`,
  `docs/content/docs/{architecture,crate-map,sessions}.mdx`, `wiki/09-architecture.md`, `sync_cargo.sh`.
  Comptes rendus historiques inchangés.
- Les copies locales « … 2 » de l’utilisateur (dont celles restées dans `crates/bricks-tui/`) n’ont pas
  été touchées.

## 4. Architecture ajoutée

- `crates/cersei-agent/src/control/slash.rs` : registre unique (noms, alias, arguments, descriptions
  EN/FR, `parse`, `complete`), sans dépendance de rendu.
- Contrat (`contract.rs`) : `SlashCommandInfo`, `SlashAction`, `SlashPresent`, `SessionToolInfo`,
  `McpServerInfo`, `SessionConfigInfo`, `ConfigEntry`, `ProjectFileEntry`. Commandes :
  `slash_commands`, `slash_parse`, `session_tools`, `session_mcp`, `session_config`, `session_locate`,
  `app_quit`, `project_files`.
- Frontend : `state/ui.svelte.ts` (vue d’inspection ouverte), `features/commands/InspectorHost.svelte`,
  réducteur complété (`turnUsages`, `usageTotalFull`, `runUsages`, `jobs`, `searchResult`,
  `agentProfiles`, `skillsListing`, `memory`, `lastCompaction`, `lastClear`, `lastSkill`,
  `lastAgentControl`) ; le réducteur ignore désormais les événements d’une autre session.

## 5. Agents, modèle et effort

Chaque lancement a imposé `model: haiku` et `effort: xhigh`. **Preuve d’exécution** : dans les
métadonnées des transcripts de chaque agent lancé pour ce sprint, toutes les requêtes portent
`"model":"claude-haiku-5-5"` et `"effort":"xhigh"` (aucune autre valeur relevée). Les agents eux-mêmes
ne voient pas l’effort dans leur prompt, d’où leurs mentions « non visible ».

| Agent | Lot livré | Contrôles rapportés |
| --- | --- | --- |
| Haiku 1 | inventaire TUI, registre `control::slash` (12 tests), IPC slash (7 tests), index `@` (11 tests) | cargo test ciblés, TUI 59 tests avant retrait |
| Haiku 2 | « Général », retrait des contrôles de disposition, checklist | MCP Svelte, check, tests unitaires |
| Haiku 3 | placeholder, palette `/`, dispatch, aide, historique, `@` mentions, `/quit` | MCP Svelte, tests unitaires (palette, historique, dispatch, mentions) |
| Haiku 4 | vues Recherche, Contexte, Vider, Consommation, Mémoire | MCP Svelte, check |
| Haiku 5 | vues Profils d’agents, Sous-agents, Skills, Tâches, ChangeSet ; confirmation d’arrêt d’agent | MCP Svelte, check |
| Haiku 6 | adaptateurs Rust outils/MCP/config/locate/quit (11 tests, dont absence de secret) ; vues Session, Reprise, Outils, MCP, Config | MCP Svelte, cargo test, check |
| Haiku 7 | correction d’une course PTY (shell non récolté si le consommateur refuse), retrait de la TUI et adaptation CLI/docs | 3 passes complètes `cargo test -p bricks-desktop`, tests CLI, `cargo tree -i bricks-tui` |
| Haiku 8 | matrice Rust `slash_mapping` (12 tests), couverture du dispatch, tests du réducteur 14.1 et isolation des sessions, checklist | test négatif de contrôle, tsc des tests |

Orchestrateur : contrats, store `ui`, hôte des vues, réducteur, client IPC, dépendances, invalidation de
l’index `@`, garde de session du réducteur, ouverture du panneau pour `/diff`, références obsolètes.

## 6. Contrôles effectués

- `npm run check` : 0 erreur (1 avertissement préexistant `baseUrl`). `npm run test:types` : OK.
- `npm run test:unit` : 226/226. `npm run build` : OK.
- `cargo test -p bricks-desktop` : 97 tests unitaires + 12 `slash_mapping` (3 passes complètes vertes).
- `cargo test -p cersei-agent --lib control::` : 22. `cargo test -p bricks-cli --bin bricks --test headless
  --test system_prompt --test workspace --test pty_headless` : 5, 8, 1, 1, 5.
- `cargo tree -i bricks-tui` : absent du graphe. `bricks` sans argument → aide, code 0 ; `bricks tui`,
  `bricks resume`, `--hyperlinks` → refusés (code 2).
- Bundle macOS debug `Bricks.app` construit et ouvert.

## 7. Non vérifié et limites

- **Parcours dans l’application démarrée : non vérifiés.** La capture d’écran automatisée échoue sur
  cette machine (« problème de capture audio/vidéo ») ; une seule tentative a été faite. La lecture de
  l’arbre d’accessibilité via System Events renvoie une liste vide (permission d’accessibilité absente
  pour ce terminal). Restent à contrôler à la main (checklist `tests/README.md`, entrées marquées
  « non vérifié ») : placeholder, « Général », panneaux manipulables et mémorisés, palette au clavier,
  chaque commande migrée et son effet observable, un outil avec approbation, une annulation, une reprise
  `/resume <id>` entre projets, `/quit` pendant une exécution, `@` mentions.
- La condition « reprise vérifiée avant suppression » est remplie par les tests automatisés (matrice,
  couverture, réducteur) mais pas par un parcours natif ; la TUI reste récupérable dans l’historique Git.
- `cargo build --workspace` échoue sur deux copies utilisateur non suivies avec espace dans le nom
  (`bench/long-mem/src/bin/longmem_recall 2.rs`, `crates/cersei-vms/src/bin/cersei_envd 2.rs`) ; build
  vert en excluant ces deux crates. Non touchées (hors périmètre).
- Les textes renvoyés par le moteur (`agent_control_result`, refus) restent en anglais, affichés comme
  texte du moteur. L’index `@` n’expose pas d’indicateur « index tronqué ». Quirks conservés de la TUI :
  `/search re:` vide envoie une expression vide, `/agents status` sans id est une recherche de profils.
- Windows / Linux : non testés.
- Dépendances directes déjà inutilisées dans `bricks-cli` avant ce sprint (`cersei-tools`,
  `cersei-embeddings`, `tokio-util`, …) : laissées en place.
