# Sprint 14.3 — Nettoyage interne et accès ADK : walkthrough

Branche `Bricks_Desktop`, base `e62a6be`. Rien n’est commité ni poussé. `system_prompt.rs` (modification
utilisateur), `bricks.toml` et `docs/sprint-14*.md` non touchés. Walkthroughs 14, 14.1 et 14.2 préservés.
Rapports détaillés : `docs/cleanup-14-3.md`, `docs/duplicates-14-3.md`, `docs/adk-matrix-14-3.md`,
revue `tests/REVIEW_14_3.md`.

## 1. Nettoyage

**Code Rust interne** (`cleanup-14-3.md`) : `git_utils` (dix blocs `git` mutualisés, signatures et règles de
statut inchangées, nouveau test), `file_history` (helper privé commun), helper de test mort supprimé, attributs
`allow` réduits de 9 à 4 (tous justifiés, au niveau du champ). Gain net vérifié : **−19 lignes** sur 9 fichiers.
**Toutes les API publiques conservées** : `file_snapshot`, `file_watcher`, `vm_tools`, `lsp_tool`,
`code_intel`, `auto_dream`, `coordinator`, `session_memory` et les symboles publics isolés (exemples, features,
documentation). Défauts préexistants relevés, non corrigés car ce serait changer le comportement du SDK :
`append_system_prompt` sans effet, `AgentStream::respond_permission` / `inject_message` ignorés (limitation
documentée), entrée `prompt` de TaskCreate ignorée.

**Copies « … 2 »** (`duplicates-14-3.md`), recalculées au moment d’intervenir : 677 copies non suivies —
633 identiques à l’original, 20 différentes mais retrouvées à l’identique dans l’historique Git, 24 sans original
retrouvées dans l’historique (dont tout le résidu `crates/bricks-tui/`). **Aucune version perdue** : 0 copie à
sauvegarder. Retrait par chemins explicites ; 3 dossiers vides supprimés. Une copie ignorée par `.gitignore`
(`bench/term-bench/runner-google 2.sh`, identique) et les entrées internes de `.git/` laissées en place.
Conséquence : `cargo build --workspace` et `cargo test --workspace --no-run` passent sans exclusion.

## 2. Interface

- **Modèles** : « Nom API », entrée et sortie maximales ; champ fenêtre de contexte retiré du formulaire (valeur
  du fichier préservée octet pour octet, jamais envoyée). Profils d’effort en petites cases + `+` : identifiant,
  libellé, objet JSON `parameters`, chemins `remove` repliés ; doublons, JSON invalide, défaut retiré/renommé
  traités sans écriture. Rust : remplacement ciblé `toml_edit`, validation `cersei-provider` avant écriture
  atomique ; test de bout en bout : un profil au nom libre sauvegardé, rechargé puis réellement transmis dans le
  corps de requête (serveur HTTP local).
- **Apparence** : Accent, Fond principal, Fond des panneaux, chacun avec une palette visuelle (zone
  saturation/luminosité, teinte, nuancier, aperçu, contraste), par mode résolu, appliqués aux variables CSS et
  persistés ; « Affichage actuel » retiré. **Geist Sans** (interface) et **Geist Mono** (code, terminal) embarquées
  localement avec leur licence OFL ; Inter retirée.
- **Textes parasites** retirés : historique et onglet Conversation des modifications, composeur (infobulle
  « Envoyer »), Skills, Modèles, Apparence, 19 vues d’inspection.
- **Bilan d’exécution** : `Terminé · 762 ms · +120 −24` — durée depuis les horodatages moteur, `+/−` seulement
  depuis les éditions structurées ; erreurs sur une seconde ligne.
- **En-tête** : grille à colonnes latérales égales, titre et projet centrés, repli gauche en haut à gauche,
  un seul point de renommage, zones de glisser-déplacer Tauri sur les parties vides.
- **Saisie et liens** : `dir="auto"` + `text-start` (composeur et bulle utilisateur) ; URL `http(s)` et `www.`
  des messages utilisateur rendues en nœuds Svelte (pas de `{@html}`), code exclu ; Markdown assistant inchangé
  (marked + DOMPurify), liens inactifs pendant le streaming.
- **Terminaux** : cause reproduite (hôte xterm jamais détaché) et corrigée : un seul terminal visible, PTY et
  historique conservés au changement d’onglet ; géométrie recalculée depuis le conteneur visible (onglet,
  panneau, redimensionnement, polices) ; hauteur en proportion (`terminal_ratio`, 0,1–0,8, migrée une fois depuis
  l’ancienne valeur en px, bornée côté Rust).

## 3. Matrice ADK (résumé ; détail dans `adk-matrix-14-3.md`)

| Capacité existante | Preuve moteur | Accès UI | Effet vérifié |
| --- | --- | --- | --- |
| Sessions / reprise | `Controller`, `session_open`, `session_locate` | navigation gauche, `/resume` | tests 14.1 ; natif non vérifié |
| Modèles / efforts | `ProviderRegistry`, `ReasoningProfile` | Paramètres → Modèles, sélecteurs du composeur | tests Rust de bout en bout |
| Outils / permissions | `ApprovalRules::effective`, `Controller::tools()` | Général, panneau droit « Plus » → Outils / Configuration | tests 14.2 |
| Profils et instances d’agents | `ProfileRegistry`, `AgentRuntime::records()` | « Plus » → Profils d’agents / Sous-agents ; restaurés à la réouverture (`session_agents`) | revue de code + tests unitaires |
| ChangeSets | `workspaces.changesets()` | panneau Modifications (liens), inspecteur ; restaurés (`session_changesets`) | tests unitaires |
| Jobs | `agent_control` jobs / stop_job | « Plus » → Tâches de fond | tests 14.1 ; lecture structurée hors session impossible (`extensions` privé) |
| Mémoire / contexte | snapshot, événements `context`, `memory_*` | panneau droit, « Plus » → Contexte / Consommation / Mémoire | tests 14.2 |
| MCP | `Controller::mcp_statuses()` | « Plus » → Serveurs MCP | tests 14.1 |
| Skills | `SkillsService`, `Prompt.skills` | Paramètres → Skills (sans confiance implicite), menu `+` → Skills (puces retirables) | tests Rust catalogue + transport du `submit` |
| Recherche dans le code | `Command::Search` | « Plus » → Recherche dans le code, `/search` | tests 14.1 |

Non raccordé (absent ou privé dans le moteur, non créé) : lecture structurée des jobs hors événements,
`analyze_context` (public mais sans usage).

## 4. Agents

Huit sous-agents Haiku 5.5 (au-delà du plancher de quatre). **Preuves issues des métadonnées d’exécution** :
chaque requête de chacun porte `"model":"claude-haiku-5-5"` et `"effort":"xhigh"`, sans autre valeur.

| Agent | Lot | MCP Svelte (appels constatés : list / doc / autofixer) |
| --- | --- | --- |
| D | classement et retrait des copies « … 2 » | — (pas de Svelte) |
| R1 | nettoyage Rust interne, API préservées | — |
| R2 | profils (TOML, validation, transmission), préférences, catalogue de skills, adaptateurs `session_agents` / `session_changesets`, matrice moteur | — |
| F1 | formulaire modèles/profils, palettes, Geist, Paramètres → Skills | 1 / 1 / 15 |
| F2 | historique, bilan, en-tête, saisie RTL, liens, Skills du menu `+` | 1 / 3 / 23 |
| F3 | terminaux (détachement, géométrie, proportion) | 1 / 1 / 10 |
| F4 | accès ADK du panneau droit, textes des vues, liens ChangeSet, section « Accès UI » | 1 / 2 / 22 |
| V | validation : 2 suites Rust et 3 suites frontend ajoutées, revue | 0 / 0 / 3 (lecture seule) |

Orchestrateur : contrats (`ThemeColors`, `terminal_ratio`, `ReasoningProfileEdit`, `ModelPatch.profiles`,
`SkillsCatalog`, `draft.skills`), polices embarquées, dépendances (`bricks-skills`, retrait d’Inter),
enregistrement et hydratation des adaptateurs ADK, arbitrages de revue (V7 : composant shadcn inutilisé, laissé).

## 5. Contrôles exécutés

- `npm run check` : 0 erreur (avertissement `baseUrl` préexistant). `npm run test:types` : 0 erreur.
- `npm run test:unit` : 34 fichiers, 511 tests. `npm run build` : OK (Geist servie localement).
- `cargo build --workspace` et `cargo test --workspace --no-run` : OK, sans exclusion. `cargo build --examples -p cersei` : OK.
- `cargo test -p bricks-desktop` : 146 + 5 + 10 + 9 + 4 + 12. `cargo test -p cersei-agent --lib` : 178.
  `cargo test -p bricks-cli --bin bricks --test headless --test workspace` : 5, 8, 5. Crates nettoyées
  (rapport R1) : cersei-tools 194 + 23, cersei-types 29, cersei-provider 161.
- Build release macOS : `target/release/bundle/macos/Bricks.app`.

## 6. Non vérifié et limites

- **Parcours dans l’application démarrée : non vérifiés.** La capture automatisée échoue sur cette machine et
  n’a pas été retentée ; la checklist « Sprint 14.3 » de `tests/README.md` est marquée « non vérifié »
  (palettes et persistance, rendu Geist dans WKWebView, en-tête centré et glisser-déplacer, liens ouverts dans le
  navigateur, RTL, trois onglets de terminal avec un vrai shell, Skills avec un fournisseur réel).
- L’acceptation des paramètres libres d’un profil par un fournisseur réel n’est pas affirmée : seule la
  transmission dans la requête est testée.
- Un message composé uniquement de skills reste non envoyable (règle de contenu existante).
- Les défauts SDK préexistants listés au §1 restent ouverts.
- Windows / Linux : non testés.
