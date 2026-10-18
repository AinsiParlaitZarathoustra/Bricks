# Sprint 14.6 — Exécution concurrente et finitions du chat : walkthrough

Branche `codex/sprint-14-6-execution-ui`, base `main` @ `9b9b4da`. Modifications utilisateur préservées :
`crates/cersei-agent/src/system_prompt.rs`, `apps/bricks-desktop/tests/README.md`, fichiers locaux.

## 1. Plan initial (après audit, avant code)

Audit en lecture seule par deux Haiku 5.5 `high` (localisation ; diagnostic avec preuves exécutées).
Constats prouvés : deux `Edit` concurrents sur des zones distinctes d'un même fichier → **400/400
mises à jour perdues** ; 4 × `sleep 1` dans la même session Bash → **4 291 ms** (4 sessions : 1 118 ms) ;
trois trous d'approbation (état non revérifié après approbation, entrée modifiée par un hook sans nouvelle
décision, hash d'aperçu relu sur disque au lieu du texte du diff).

| Lot | Contrat | Propriétaire |
| --- | --- | --- |
| 1 Garde | retrait de `read_before_edit_block`, `refusals_for_batch`, `files_read` ; tests d'exécution réelle | Haiku logique 1 (`runner.rs`) |
| 2 Edit | `Edit` simple ou `edits[]` → `Vec<EditSpec>`, préparation séquentielle via `plan_edit`, une `write_atomic` ; verrou par fichier canonique (jamais pendant l'approbation) ; `ApprovedState` revérifié sous verrou ; retrait de `MultiEdit` | Haiku logique 1 |
| 3 Bash | un processus par appel ; `ShellState` (cwd, exports) instantané au lancement, delta publié à la fin par fichier d'état atomique, dernière commande lancée gagne par champ | Haiku logique 2 (`shell/`, config) |
| 4 Jobs | `yield_time_ms` distinct de `timeout`, `ToolStatus::Running`, même `Task` supervisée, deadline maintenue ; `JobRegistry::reserve()` RAII ; annulation par groupe | Haiku logique 2 (raccordement runner par logique 1) |
| UI | anneau du composeur, carte Contexte, Copier, `MarkdownView` partagé (chat + sous-agents), fichiers révélés dans le Finder, croix du terminal | Opus ; adaptateur `reveal_agent_file` et formats `Edit` : Haiku logique 3 |
| Contrôles | tests fonctionnels (Haiku high), code mort (Haiku high), documentation (Haiku medium), contrôle natif (Opus) | — |

## 2. Commits

| Commit | Contenu |
| --- | --- |
| `918dc66` Remove the mandatory Read before file edits | garde, `files_read`, refus par lot retirés ; tests d'exécution réelle (`edit_runtime.rs`) ; consignes adoucies (`file_read.rs`, `backend_coder.md`, en-tête skeleton) |
| `4263cd9` Run each Bash call in its own process | `shell/state.rs`, `shell/{bash,background,mod,procs,capture}.rs`, `driver.sh` supprimé, `bash.rs`, `jobs.rs`, `[bash]`, raccordement `controller.rs` |
| `18ac0d4` Merge MultiEdit into Edit and serialise edits per file | `file_edit.rs` (`EditSpec`, `parse_call`, `prepare`), `file_lock.rs`, `preview.rs` (`ApprovedState`), `file_write.rs`, `notebook_edit.rs`, `fs.rs` (`write_target`), `runner.rs` (`decide`, `decide_and_run`, arrêt des commandes ordinaires en fin de run), `multi_edit.rs` supprimé, CHANGELOG |
| `8809a89` Desktop: context ring, session usage, answer copy, sub-agent details | interface (Opus) + `reveal_agent_file` et formats `Edit` (Haiku logique 3) |
| `3494b12` Fix clippy warnings in tests and desktop helpers | lints préexistants, sans changement de comportement |
| docs | `docs/shell.md` réécrit, `docs/{agents,cli,compression}.md`, mdx, wiki, README, ce walkthrough, checklist |

Chaque commit moteur a été vérifié dans un worktree détaché propre (`cargo test -p cersei-tools -p
cersei-agent` vert, `cargo check` de `bricks-desktop`, `bricks-cli`, `cersei-agentlang`). Le commit 1 vient
d'un patch « lot 1 seul » reconstruit depuis `HEAD` par le codeur propriétaire de `runner.rs`.

## 3. Moteur

### Lecture préalable
Plus aucun refus « Call Read first » : `Edit`, `Write`, `NotebookEdit`, `ApplyPatch` s'exécutent sur un
fichier jamais lu ; `Read` et `Edit` d'un même lot sont indépendants, sans ordre promis. Permissions,
aperçus, hashes et refus d'un remplacement absent ou ambigu sont conservés. `system_prompt.rs` n'a pas été
modifié (sa consigne « Examine le code concerné avant de le modifier » n'est pas contradictoire).

### `Edit` unique
Forme simple ou `edits: [...]` (une seule forme ; liste non vide ; clés inconnues refusées, y compris dans
`edits[N]` ; anciens alias non ressuscités ; omission tolérée de `new_string` conservée). Préparation
séquentielle en mémoire par `plan_edit`, partagée par l'aperçu et l'exécution, diagnostics `edit N of M`,
une seule `write_atomic`, rien d'écrit en cas d'échec. `MultiEdit` : outil, export et schéma retirés, sans
alias exécutable ; un ancien historique qui contient ce nom se relit et s'affiche (test `edit_history.rs`) ;
migration SDK dans le CHANGELOG (`file_edit::FileEditTool`, forme `edits`).

**Concurrence.** Verrou par fichier canonique (`file_lock.rs`) partagé par tout le processus, pris dans
l'ordre trié, tenu pendant lecture → plan → écriture, jamais pendant l'attente d'une approbation. Course
prouvée avant le sprint (400/400 pertes), corrigée : deux éditions forcées à se chevaucher (barrières, deux
ordres) sont toutes deux sur disque ; liens et chemins alias → même verrou ; fichiers différents libres.
**Approbation.** `before_sha256` = hash du texte qui a produit le diff ; l'état approuvé est revérifié sous
verrou ; fichier changé après approbation ou entrée modifiée par un hook → nouvelle décision (au plus 3
tours), jamais d'écriture sur une approbation périmée. **Liens symboliques** : défaut trouvé par le testeur
(l'écriture atomique remplaçait le lien par un fichier) — corrigé, la cible change et le lien reste.

### Bash concurrent
Un processus par appel (supervision `Task` existante généralisée, aucun second gestionnaire). État de session :
cwd (`PWD`/`OLDPWD`), variables exportées, fonctions, alias ; instantané au lancement, delta publié à la fin
normale par un trap EXIT écrit dans un fichier temporaire renommé (canal séparé de stdout/stderr) ; même
champ : la commande lancée le plus récemment gagne. Interruption, timeout, `exec`, trap EXIT utilisateur,
signal → rien publié, note « environment changes not available ». `background: true` hérite sans publier.
`yield_time_ms` (`[bash]`, défaut 10 000, distinct de `timeout`) → `Running` avec `task_id`/`job_id` et
curseurs, suivi par `BashTask*`/`Job`, deadline maintenue. Quota `[background] max_jobs` réservé
atomiquement (`JobRegistry::reserve`, `JobSlot`). Fin ou annulation du run racine → commandes ordinaires
actives arrêtées (groupe, réaping) ; `background: true` survit au tour, pas à la session. Sandbox (feature
`vms`) : branche conservée en premier, `background`/`yield_time_ms` refusés explicitement.

| Mesure | Avant | Après |
| --- | --- | --- |
| 4 commandes dans la même session | 4 × `sleep 1` = 4 291 ms (sérialisées) | 4 × `sleep 2` : 2 s ≤ t < 5 s avec barrière de démarrage (test) |
| App native, vrai agent, 4 appels Bash d'un tour | — | début dans la même seconde, fin ensemble, ≈ 2,06 s chacun |
| Quota sous 40 appels concurrents (quota 32) | course `check_capacity` → `register` | jamais plus de lancements que de places (test) |

**Limites documentées** (`docs/shell.md`) : variables non exportées et options `set`/`shopt` non transmises ;
`deactivate` d'un venv ne restaure pas le PATH ; `exit` publie, `exec` non ; un démon qui se détache échappe
au nettoyage du groupe ; yield refusé faute de place → la commande reste attendue jusqu'au timeout ;
PowerShell inchangé (non concurrent, non revendiqué).

## 4. Interface (Opus)

- **Anneau du composeur** (`ContextGauge.svelte`) à gauche du sélecteur de modèle, 15 px, trait 2,2,
  remplissage gris depuis midi ; neutre sans conversation, mesure absente ou périmée, ou capacité inconnue ;
  dessin borné à 100 %, pourcentage réel conservé. Panneau au survol et au focus : « Contexte », valeur
  compacte (`10,8 k / 1 M (1,1 %)`), barre fine, « Taux de cache moyen » = cache lu / (entrée non cachée +
  cache lu + cache écrit), `—` sans compte. Comparé à la référence à échelle comparable (capture 1x contre
  référence 2x), clair et sombre.
- **Carte Contexte** : `9 968 / 1 000 000 tokens` sans `≈` ; « Consommation session » : Entrée, Sortie,
  Cache (lecture du cache seule), Requêtes, Coût (`non mesuré` si absent ; montant partiel signalé dans le
  nom accessible). `/context` lit les mêmes helpers (`contextView.ts`). Données moteur inchangées.
- **Composeur** : `rounded-2xl`, petit espace au-dessus ; texte, placeholder et contrôles inchangés.
- **Copier** sous chaque réponse non vide : `Copy` → `Check` 1,5 s, nom accessible « Copier », source
  Markdown de la réponse seule ; erreur de presse-papiers affichée.
- **Sous-agents** : `MarkdownView.svelte` extrait d'`AssistantMessage` (même pipeline 14.5, ids préfixés
  `a<len>-<id>-task|result`) ; « Identifiant » et « Parent » retirés de l'affichage ; sections vides masquées ;
  description non dupliquée ; « Fichiers modifiés » = noms cliquables (chemin relatif en infobulle) →
  `reveal_agent_file` (Rust : agent de la session ouverte, racine du workspace partagé ou du worktree
  enregistré, chemin canonique, liens sortants refusés, fichier supprimé → dossier parent ; opener
  `reveal_item_in_dir`, rien n'est ouvert). `summary_truncated` → « Résumé tronqué ».
- **Terminal** : croix « Masquer le terminal » juste après `+`, même état et persistance que le bouton
  supérieur (`AppShell.hideTerminal`) ; les croix d'onglet ferment toujours leur session.

## 5. Contrôles

- `cargo test -p cersei-tools` 286 ✓, `cargo test -p cersei-agent` 373 ✓, `cargo test -p bricks-desktop` ✓
  (185 unitaires + intégration), dans l'arbre et dans un worktree propre par commit.
- `cargo clippy --workspace --all-targets -- -D warnings` ✓ (après le commit de lints).
- `cargo fmt --all -- --check` : **échoue déjà sur la base `9b9b4da`** (261 blocs). Les fichiers du sprint
  sont formatés ; les seuls nouveaux écarts sont dans `system_prompt.rs` (modification utilisateur non
  commitée) et `crates/cersei-tools/tests/repro_high_bugs.rs` (fichier utilisateur non suivi), non touchés.
- Checkout propre : `svelte-check` 0 erreur, Vitest 50 fichiers / 819 tests ✓. Checkout utilisateur :
  1 erreur dans la copie non suivie `InspectorHost 2.svelte` (vue `file` absente), non touchée.
- Build release macOS `Bricks.app` 38,9 Mo, ouvert.
- **App native (computer use)**, projet de test `bricks-test-146` (dossier temporaire, aucune conversation
  réelle utilisée) : `Edit` sans lecture puis `Edit.edits` (fichier vérifié : `x = 2`, `y = 4`, `z = 6`) ;
  quatre Bash parallèles (même seconde de début) ; sous-agent : mission et résultat en Markdown (tableau,
  code coloré), « notes.md » révélé dans le Finder ; anneau et panneau (clair/sombre, thème utilisateur
  restauré) ; carte Contexte ; Copier (presse-papiers relu : source Markdown complète) ; croix du terminal
  (sortie, onglet et hauteur conservés après réouverture).

## 6. Modèles et MCP

Métadonnées d'exécution (pas d'auto-déclaration) : tous `claude-haiku-5-5`. Audit localisation `high`,
audit diagnostic `high`, logique 1 (garde, `Edit`, runner) `xhigh`, logique 2 (Bash, jobs, config) `xhigh`,
logique 3 (Finder, formats) `xhigh`, testeur `high`, code mort `high`, documentation `medium`. Opus :
orchestration, interface, intégration, commits, contrôle natif. MCP Svelte appelé par Opus :
`list-sections` (session), `get-documentation` (`$bindable`, `{#snippet}`), `svelte-autofixer` sur
`MarkdownView`, `ContextGauge`, `AgentInspector`, `AssistantMessage`, `TerminalPanel`, `ContextCard`
(remarques restantes : `$effect` d'E/S voulus, `{@html}` de contenu nettoyé).

## 7. Défauts ouverts / non vérifiés

- Fidélité de l'anneau : comparaison faite sur une capture 1x ; une capture Retina 2x n'a pas été obtenue.
- Non vérifiés en natif : annulation d'un run avec une commande rendue anticipativement (couvert par
  `bash_run_lifetime.rs`), révélation d'un fichier supprimé ou d'un worktree isolé (tests Rust), clic
  clavier sur l'anneau, zoom de la fenêtre. Windows/Linux et PowerShell non validés.
- Au rechargement d'une conversation, « Consommation session » repart à 0 (compteurs du contrôleur ouvert) :
  comportement antérieur, conservé.
- Le projet de test `bricks-test-146` reste dans la liste des projets de l'app (retrait possible depuis son
  menu ; le dossier est temporaire).
