# ADK matrix, Sprint 14.3 (desktop Rust side)

Scope: existing Bricks engine capabilities (`crates/cersei-agent`, `crates/cersei-provider`,
`crates/bricks-skills`) and whether the desktop reaches each one over IPC. No new business
capability. Read-only grep of the frontend for the UI column; the UI itself is not verified here.

IPC paths used:
- `session_command(session_id, Command JSON)`: any engine `Command`; every engine `Event` is forwarded
  unchanged as `DesktopEvent::Engine` (`engine/live.rs` `ingest`, serialises the whole envelope).
- Inspection commands in `commands/inspect.rs`, `commands/sessions.rs`, `commands/changes.rs`,
  `git/commands.rs`, `commands/providers.rs`, `commands/app.rs`.
- New in this sprint: `commands/adk.rs` (`session_agents`, `session_changesets`),
  `services/providers_toml.rs` (reasoning profiles in `model_update`), `commands/skills.rs`
  (`skills_catalog`, currently answers "unavailable": see the skills row and open item 4).

| Capability | Engine proof (file : item) | IPC path | UI consumer (grep, read-only) | Status |
| --- | --- | --- | --- | --- |
| Sessions: list, search, open, resume, rename, delete, close | `control/controller.rs` `Controller::open` (`SessionChoice`); `control/protocol.rs` `Command::Resume` | `sessions_list`, `sessions_search`, `session_open`, `session_rename`, `session_delete`, `session_close`, `session_locate`. `Resume` through `session_command` is refused on purpose (`engine/sessions.rs` `send_command`) | `features/navigation`, `app.resumeSession` | Complete |
| Models and efforts | `controller.rs` `impl ModelCatalog for ProviderRegistry` (`models`, `build`); `protocol.rs` `Command::SetModel`, `Command::SetReasoning` | `models_list`; `session_command` `set_model` / `set_reasoning`; `session_config` shows the effective profile | `features/composer/Composer.svelte`, `features/settings/ModelSettings.svelte` | Complete |
| Reasoning profiles (free names, parameters, remove) | `cersei-provider/src/reasoning.rs` `select_profile`, `apply`; `provider.rs` `request_body` reads `REASONING_PROFILE_OPTION`; `cersei-agent/src/runner.rs` sets that option per request | `providers_config` (view with `parameters`, `remove`), `model_update` with `profiles` (this sprint) | `features/settings/ModelSettings.svelte` | Complete. Transmission proven by `commands/providers/tests.rs` `a_saved_profile_with_free_names_reaches_the_request_it_selects` (local HTTP server, real request body) |
| Tools and permissions | `controller.rs` `tools()`, `snapshot()` (`approval_rules`, `allowed_for_session`, `approval_mode`); `protocol.rs` `Command::Approve`; `engine/mod.rs` `approval_mode` | `session_tools` (`inspect.rs` `tools_of`), `session_config`, `session_snapshot`, `prefs_set` (`tool_approval`), `session_command` `approve` | `features/session-info/ToolsView.svelte`, `features/conversation/timeline/ApprovalCard.svelte` | Complete |
| Project skills trust | `protocol.rs` `Command::TrustProjectSkills`; `SkillsTrustRequested` / `SkillsTrustResolved` events | `session_command` `trust_project_skills`; events | `features/conversation/timeline/SkillsTrustBanner.svelte` | Complete |
| MCP servers | `controller.rs` `mcp_statuses` | `session_mcp` (`inspect.rs` `mcp_of`) | `features/session-info/McpView.svelte` | Read-only: the engine has no command to reconnect or enable a server, so none is added |
| Agent profiles | `protocol.rs` `Command::ListAgentProfiles`, `ReloadAgentProfiles`; `Event::AgentProfiles` | `session_command` `list_agent_profiles` / `reload_agent_profiles`; events | `features/catalog/AgentProfilesView.svelte` | Complete |
| Sub-agent instances | `agents/runtime.rs` `AgentRuntime::records` (`InstanceRecord`, durable); `controller.rs` `agent_runtime()`; `agent_control` `list` / `status` answer as text | Events `AgentSpawned`, `AgentState`, `AgentFinished`; **new** `session_agents` (`commands/adk.rs`) returns the records themselves | `features/catalog/AgentInstancesView.svelte` | Gap closed at IPC level. Before: records reachable only as text or from the current run's events. Registration in `lib.rs` pending (orchestrator) |
| Sub-agent ChangeSets | `agents/workspace.rs` `WorkspaceManager::changesets`; `controller.rs` `agent_control` `inspect_changes` / `apply_changes` / `discard_changes` | `session_command` `agent_control` (text result), events `ChangesReady`, `ChangesUpdated`; **new** `session_changesets` | `features/catalog/ChangesetView.svelte` | Same as sub-agents: registration pending |
| Background jobs | `controller.rs` `job_control` (routed at `agent_control` `jobs` / `stop_job`); `JobsHandle` lives in `Agent.extensions` | `session_command` `agent_control` `jobs` / `stop_job` (text result); events `JobStarted`, `JobOutput`, `JobFinished` | `features/catalog/JobsView.svelte` | **Partial, engine accessor missing.** `Agent.extensions` is `pub(crate)` (`lib.rs:250`), so the desktop cannot list jobs as data after the run that started them. Needs a public `Controller::jobs()`; not added (engine file, not owned) |
| Memory | `controller.rs` `Snapshot.memory_space`, `maintenance_running`; maintenance starts after a completed run (`controller.rs` around `MemoryMaintenanceStarted`); `Command::Cancel` stops it | `session_snapshot`, `session_config` (`memory_space`); events `MemoryRecalled`, `MemoryMaintenanceStarted` / `Finished` | `features/inspect/MemoryView.svelte` | Complete for what the engine exposes. No manual trigger exists in the engine, none added |
| Context and compaction | `context.rs` `ContextStatus` (in `Snapshot.context`); `protocol.rs` `Command::Compact`, `Command::ClearContext`; `Event::Context`, `Compaction`, `ContextCleared` | `session_snapshot`; `session_command` `compact` / `clear_context`; events | `features/inspect/ContextView.svelte`, `ClearView.svelte` | Complete. `context_analyzer.rs` `analyze_context` (per-category estimate) is public but no runtime path calls it; not wired here (it is an estimate, and the status above already covers the budget) |
| Cost and usage | `Snapshot.usage`; `Event::Usage`, `Event::RunUsage` | `session_snapshot`; events | `features/...` cost views (`/cost` opens `cost`, `cost_details`) | Complete |
| Code search | `protocol.rs` `Command::Search`; `Event::SearchResults` | `session_command` `search`; `project_files` (`@` mentions) | `features/inspect/SearchView.svelte` | Complete |
| Skills: catalogue and sources | `bricks-skills` `SkillsService`, `Registry`, `Trust`; `protocol.rs` `Command::ListSkills`, `ReloadSkills`; `Event::Skills` | `session_command` `list_skills` / `reload_skills` inside a session (complete). Outside a session: `skills_catalog` is registered in `lib.rs` but **answers "unavailable"**: its service (`services/skills.rs`, written and tested out of tree) needs `bricks-skills` as a direct dependency of `bricks-desktop`, which is not declared | `features/settings/SkillsSettings.svelte`, `features/catalog/SkillsView.svelte`, `features/composer/SkillPicker.svelte` | **Blocked on a manifest change** (open item 4) |
| Skills: activation on send | `protocol.rs` `Prompt.skills`; `Command::Submit` | `session_command` `submit` with `skills` | `features/composer` | Complete |
| Changes and Git | `services/changes.rs` (journal), `git/commands.rs` | `change_log_list`, `change_log_diff`, `change_log_info`, `git_status`, `git_diff`, `git_log`, `git_show`; event `ChangeRecorded` | `features/changes` | Complete |
| Slash registry | `cersei-agent/src/control/slash.rs` `COMMANDS` | `slash_commands`, `slash_parse` | `features/commands/dispatch.ts` | Complete |

## Open items (not done in this sprint)

1. **Registration (orchestrator)**: add to `src-tauri/src/lib.rs` the handler list
   `commands::adk::session_agents` and `commands::adk::session_changesets`, and to `src/lib/ipc/client.ts`
   the matching functions (`session_agents(session_id)`, `session_changesets(session_id)`, both returning
   JSON arrays of the engine's `InstanceRecord` and `ChangeSet`). Both return `serde_json::Value`, so
   no contract type is needed.
2. **Jobs as data**: a public `Controller::jobs()` in `cersei-agent` (wrapping `JobsHandle::list`) would let
   the desktop list jobs after a reopen. Not done: engine code is outside this lot.
3. **Context analyzer**: `analyze_context` is public and unused by the product. Decide whether the context view
   should show its per-category estimate; not wired.
4. **Skills catalogue (orchestrator, manifest)**: add `bricks-skills = { workspace = true }` under `[dependencies]`
   of `apps/bricks-desktop/src-tauri/Cargo.toml` (the workspace already declares it; `cersei-agent` does not
   re-export the crate). Then: add `pub mod skills;` to `src/services/mod.rs`; add to `AppState`
   (`engine/mod.rs`) a field `pub(crate) skills: Arc<SkillsCatalogStore>` initialised with
   `SkillsCatalogStore::new(dirs::home_dir())`; replace the body of `commands/skills.rs` `skills_catalog` with
   a call to `state.skills.catalog(project_id, project.as_deref(), &query, page, reload)` inside
   `spawn_blocking`. The 8 tests in `services/skills.rs` pass in an out-of-repo crate that depends on
   `bricks-skills` by path, so they only need the module to be declared to run in the desktop crate.

## Accès UI (frontend, F4)

Ajouté par l’agent frontend F4, en complément de la table moteur ci-dessus (qui n’est pas modifiée ici). Chaque ligne indique la zone et le contrôle qui exposent la capacité, puis le mode de vérification de l’effet. Modes : **test unitaire** (exécuté), **revue de code** (câblage lu dans le source), **non vérifié nativement** (aucun parcours lancé dans l’application macOS).

Le panneau droit porte le menu « Plus » (`features/context/ContextPanel.svelte`). Il ouvre les vues existantes par `ui.open(view)` ; aucune action n’est dupliquée. Arrêt, compaction, vidage, application, abandon et approbation restent dans leurs vues ou dans la conversation.

| Capacité existante | Accès UI (zone · contrôle) | Effet vérifié |
| --- | --- | --- |
| Session : identifiant, dossier, modèle, effort, activité, mémoire, approbations | Panneau droit · Plus · Conversation · *Session* (`session`) | Revue de code ; non vérifié nativement |
| Outils et permissions | Panneau droit · Plus · Conversation · *Outils* (`tools`) | Revue de code ; non vérifié nativement |
| Serveurs MCP | Panneau droit · Plus · Conversation · *Serveurs MCP* (`mcp`) | Revue de code ; non vérifié nativement |
| Configuration effective | Panneau droit · Plus · Conversation · *Configuration* (`config`) | Revue de code ; non vérifié nativement |
| Sous-agents : liste, détail, arrêt (confirmation) | Panneau droit, section « Sous-agents » ; Plus · Agents · *Sous-agents* (`agent_instances`). Restaurés à la réouverture par `session_agents` (`app.openSession` → `hydrateAgents`) | Revue de code ; non vérifié nativement |
| Profils d’agents | Plus · Agents · *Profils d’agents* (`agents`) | Revue de code ; non vérifié nativement |
| Tâches de fond : état, arrêt (confirmation) | Plus · Agents · *Tâches de fond* (`jobs`). Liste issue des événements de la conversation : non restaurée après réouverture (accesseur moteur manquant, point 2 ci-dessus) | Revue de code ; non vérifié nativement |
| Approbations en attente (lecture) | Panneau droit, bloc « Approbations en attente » ; la décision reste dans la conversation (existant) | Revue de code ; non vérifié nativement |
| Occupation du contexte, compaction, vidage | Plus · Contexte et mémoire · *Contexte* (`context`) ; « Vider le contexte… » (`clear`) | Revue de code ; non vérifié nativement |
| Consommation et coût de la session, par tour | Plus · Contexte et mémoire · *Consommation* (`cost`) | Revue de code ; non vérifié nativement |
| Mémoire à long terme : espace, dernier rappel, maintenance | Plus · Contexte et mémoire · *Mémoire* (`memory`) | Revue de code ; non vérifié nativement |
| Recherche texte dans les fichiers du projet | Plus · Contexte et mémoire · *Recherche dans le code* (`search`) | Revue de code ; non vérifié nativement |
| Skills : catalogue, rechargement, confiance du projet | Paramètres · *Skills* (`SkillsSettings`) ; composeur · menu `+` · *Skills…* (`SkillPicker`) | Revue de code ; non vérifié nativement. Voir limite 3 |
| Modifications isolées (ChangeSets) : appliquer, abandonner | Inspecteur de sous-agent, section « Modifications isolées » (confirmation) ; restaurées à la réouverture par `session_changesets` | Revue de code ; non vérifié nativement |
| Modifications isolées : inspection détaillée du diff | Panneau Modifications, ligne du fichier, bouton « Voir les modifications isolées » (`ConversationChanges.svelte` → `ui.open('changeset', { changesetId })`, identifiants par `catalog/changeset-links.ts`) ; `/changeset` reste disponible | Test unitaire : `tests/unit/changeset-links.test.ts`, 4 tests passés ; câblage par revue de code ; non vérifié nativement |
| Reprise de session | Navigation gauche (clic sur une conversation) ; `/resume` (`app.resumeSession`) | Revue de code ; non vérifié nativement |
| Composition du menu « Plus » (chaque vue une fois, libellés non vides, libellés = titres des fenêtres) | `features/context/adkAccess.ts` | Test unitaire : `tests/unit/adk-access.test.ts`, 3 tests passés |

Le menu « Plus » est désactivé sans conversation ouverte : toutes ses vues lisent la conversation affichée.

### Limites restantes

1. **Modifications isolées dans le panneau Modifications** : fermé dans `ConversationChanges.svelte`. Chaque fichier dont les opérations appartiennent à un changeset affiche un bouton « Voir les modifications isolées » (un par changeset si plusieurs) qui ouvre `ChangesetView`. Reste ouvert hors de ce lot : `HistoryChanges.svelte` (autre propriétaire) affiche encore l’identifiant du changeset sans lien.
2. **Tâches de fond après réouverture** : `JobsView` lit les événements de la conversation. Une réouverture ne restaure pas les tâches tant que l’accesseur public `Controller::jobs()` n’existe pas (point 2 de la table moteur).
3. **Ligne Skills de la table moteur** : elle indique encore « Bloqué sur un changement de manifeste ». Le code actuel est câblé : dépendance `bricks-skills` dans `src-tauri/Cargo.toml`, `AppState.skills` (`engine/mod.rs`), `skills_catalog` qui appelle `SkillsCatalogStore::catalog`. Ce câblage n’a pas été exécuté ; la ligne doit être confirmée ou corrigée par l’agent Rust.
4. **Parcours natifs non lancés** : menu, fenêtres, arrêt d’un agent ou d’une tâche, compaction, vidage, application ou abandon d’un changeset, reprise de session et restauration après réouverture ne sont **pas vérifiés** dans l’application.

## Update: skills catalogue wired

Open item 4 is done. `bricks-skills` is a dependency of `bricks-desktop`; `services/skills.rs` is declared in
`services/mod.rs`; `AppState.skills` holds a `SkillsCatalogStore`; `commands/skills.rs::skills_catalog` reads
through it. The 8 tests in `services/skills.rs` run in `cargo test -p bricks-desktop` (146 lib tests pass).
The skills row above that says "unavailable" is superseded by this note.
