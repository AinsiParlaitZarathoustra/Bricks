# Sprint 14.2 — Une interface plus légère : walkthrough

Branche `Bricks_Desktop`, au-dessus des changements locaux non commités du Sprint 14.1 (préservés).
Rien n’est commité ni poussé. `system_prompt.rs` et les fichiers utilisateur « … 2 » non touchés.

## 1. Changements

**Contexte.** Le bloc d’occupation devient `≈ N / M tokens` : `context_used.tokens` quelle que soit la
provenance (`measured`, `counted`, `mixed`, `estimated`), sur la fenêtre totale du modèle
(`context_window.total`, jamais remplacée par la limite d’entrée). Valeur absente → `—` ; mesure périmée
(changement de modèle, compaction, vidage) → `—` pour les deux nombres jusqu’au prochain événement
`context`. Anneau, provenance, méthode, calibration, sortie réservée et notes retirés de l’affichage
(données moteur inchangées). La vue `/context` réutilise la même ligne, avec ses actions Compacter /
Vider. « Consommation cumulée (facturée) » garde ses sept catégories ; seule la phrase « Ces totaux
additionnent… » est retirée.

**Appels d’outils.** Une ligne légère : nom réel de l’outil (événement moteur), durée en ms depuis
`duration_ms` (rien si absente), état « En cours » / « Terminé » / « Échec » / « Interrompu » (icône +
texte). Commande, arguments, entrées/sorties JSON et carte imbriquée retirés ; même ligne pour les outils
des sous-agents. Approbations en attente inchangées (actionnables, aperçu) ; une fois résolues, une ligne
(« Autorisé · Bash · par vous »).

**Moins de cadres.** Sélecteurs de modèle et d’effort en style fantôme, pièces jointes sans bordure,
une seule enveloppe autour de la saisie (anneau de focus `focus-within`). Composants shadcn de base non
modifiés.

**Paramètres en bas à gauche.** Pied fixe « Paramètres » sous les listes de la barre gauche
(`ui.settingsOpen`) ; bouton de la barre supérieure retiré. En petite fenêtre, il est dans le panneau
temporaire.

**Approbation des outils (Général).** Standard / Toujours demander / Sans confirmation, persisté dans les
préférences desktop, enregistré immédiatement (annulé si l’écriture échoue). Effet réel moteur :
`ApprovalRules::effective` (une seule fonction) — `forbidden` et `deny` explicite refusent toujours ;
« Toujours demander » demande et ignore les autorisations de session (et « Autoriser pour la session »
y devient une autorisation ponctuelle non enregistrée, bouton masqué) ; « Sans confirmation » autorise ;
« Standard » inchangé. Override atomique partagé par toutes les sessions ouvertes et leurs sous-agents,
appliqué aux décisions suivantes ; une demande déjà affichée n’est jamais résolue automatiquement.
`/tools` et `/config` montrent la décision effective. La CLI headless n’est pas concernée.

**Modèles.** Un contenant par fournisseur : en-tête (nom, crayon, corbeille rouge), puis un modèle par
ligne (nom lisible seulement). Clic sur le nom → détail replié. Crayon → formulaire (fournisseur : nom,
endpoint, protocole, authentification, en-tête, nom de variable de clé ; modèle : nom, modèle d’API,
limites, fenêtre, modalités, profil par défaut). Corbeille → confirmation (celle du fournisseur nomme ses
modèles). Rust : lecture du fichier réellement chargé, `revision` (sha256) et refus `conflict` si le
fichier a changé, édition ciblée `toml_edit` (commentaires et champs non modifiés préservés, secrets
jamais exposés), validation du fichier complet par `cersei-provider` avant écriture atomique, puis
rechargement du catalogue (registre interchangeable). Un run actif garde son fournisseur ; les changements
s’appliquent au prochain envoi ; un modèle retiré est refusé au prochain envoi, sans substitution. Phrases
« Les clés d’API ne sont jamais affichées » et « modifiez le fichier puis relancez » retirées.

## 2. Agents (quatre Haiku 5.5)

| Agent | Lot | Contrôles rapportés |
| --- | --- | --- |
| Haiku 1 — Contexte et activité | `contextView.ts`, ContextCard, ContextView, `ToolLine`, ApprovalCard, AgentInspector, icônes par import direct, masquage de l’autorisation de session | MCP Svelte, check, tests unitaires |
| Haiku 2 — Interface et paramètres | composeur sans cadres, pied « Paramètres », réglage d’approbation, page Modèles et formulaires, gestion des conflits, champs requis | MCP Svelte, check, tests unitaires |
| Haiku 3 — Intégration Rust | override d’approbation moteur, catalogue rechargeable, `providers_*`, préférences tolérantes champ par champ | cargo test moteur / desktop / CLI |
| Haiku 4 — Validation | tests Rust (`approval_override` 7+1, `providers_edit` 9, `preferences_approval` 5) et frontend (19), revue `tests/REVIEW_14_2.md`, checklist | tous les contrôles ci-dessous |

Orchestrateur : contrats (`ToolApproval`, `ProvidersFile`/`ProviderView`/`ModelView`/patchs, code
`conflict`, `ui.settingsOpen`, `app.reloadModels`), retrait du bouton supérieur, `approval_mode` dans
`engine.ts`, enregistrement immédiat avec rollback, arbitrages de la revue (M2 conforme à la consigne :
accès dans le panneau gauche après ouverture ; L7 : bloc cumulé conservé volontairement).

**Preuves modèle / effort / MCP** (métadonnées des transcripts, pas déclaration des agents) : chaque
requête des quatre agents porte `"model":"claude-haiku-5-5"` et `"effort":"xhigh"`, sans autre valeur.
Appels MCP Svelte observés : Haiku 1 et Haiku 2 — `list-sections`, `get-documentation`,
`svelte-autofixer` (répétés après corrections) ; Haiku 4 — `svelte-autofixer` en lecture seule ;
Haiku 3 — aucun (Rust uniquement).

## 3. Contrôles exécutés

- `npm run check` : 0 erreur (1 avertissement préexistant `baseUrl`). `npm run test:types` : 0 erreur.
- `npm run test:unit` : 18 fichiers, 327 tests.
- `cargo test -p bricks-desktop` : 122 + 5 + 9 + 12. `cargo test -p cersei-agent --lib control::` : 30.
  `--test approval_override` : 8. `cargo test -p bricks-cli --bin bricks --test headless --test workspace` :
  5, 8, 5.
- Tests sur configurations et projets temporaires uniquement ; le catalogue réel n’a pas été modifié.
- Bundle macOS debug reconstruit et ouvert.

## 4. Non vérifié et limites

- **Parcours dans l’application démarrée : non vérifiés.** La capture automatisée échoue sur cette
  machine (erreur de capture audio/vidéo, déjà à l’origine d’un plantage) et n’a pas été retentée ;
  l’arbre d’accessibilité n’est pas lisible depuis ce terminal. Checklist manuelle : `tests/README.md`
  (section Sprint 14.2, entrées « non vérifié »).
- Le modèle extracteur de mémoire (`[memory] extractor_model`) reste résolu sur le registre chargé au
  démarrage (`bricks-config::Projects`) : une édition de ce modèle s’applique après redémarrage.
- « Toujours demander » s’applique à tout outil non refusé, y compris de niveau `none`.
- Les éditions ne prennent en charge que les tables `[[providers]]` / `[[providers.models]]` ; les
  tableaux en ligne sont refusés avec un message. Effacer le nom de variable de clé n’est pas exprimable
  par le contrat (bloqué dans le formulaire).
- `cargo test -p cersei-agent` global ne compile pas à cause de copies utilisateur `tests/* 2.rs`, et
  `documented_examples_load_cleanly` échoue sur `docs/agents/examples/* 2.md` (préexistant, non touché).
- Windows / Linux : non testés.
