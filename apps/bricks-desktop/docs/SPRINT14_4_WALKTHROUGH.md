# Sprint 14.4 — Corrections chirurgicales de l'interface : walkthrough

Branche `Bricks_Desktop`, base `08457fb`. Préservés : `6f12d22`, `08457fb`, la modification utilisateur de
`system_prompt.rs`, les données de `~/.bricks`.

## 1. Plan initial (constat → fichier → changement → contrôle)

Audit en lecture seule par un Haiku (effort `high`, choix de l'utilisateur à la place de « Luna », modèle
indisponible dans ce lanceur), recoupé dans les sources.

| Constat | Fichier | Changement | Contrôle |
| --- | --- | --- | --- |
| La bordure basse de la barre supérieure est un trait plein | `layout/TopBar.svelte:53` | `border-transparent` (géométrie inchangée) | capture native |
| Doubles traits sur les côtés : bordure de panneau + poignée de 1 px | `layout/AppShell.svelte:312,330,342` + `resizable-handle.svelte:19` | retirer les bordures de panneau, poignée `bg-border/50`, zone de prise inchangée | capture + glisser |
| Chaque clic « Nouvelle conversation » écrit une session (`Controller::open` → `meta.save`, `controller.rs:758`) | `NewConversationButton.svelte`, `state/app.svelte.ts`, `Composer.svelte` | brouillon local par projet ; une seule création, au premier envoi valide, partagée entre appelants | test 20 clics / concurrence ; liste et disque |
| `/skill nom demande` : la bulle ne montre que la demande | `Composer.svelte`, `dispatch.ts`, `conversation.svelte.ts`, `engine/live.rs` | `display` transmis avec le `submit`, étiquette liée au `run_id`, persistée côté desktop, restituée à la réouverture | tests Rust/TS + capture |
| Textes pédagogiques permanents | `App.svelte:125-131`, `ConversationTimeline.svelte:196`, `NavigationPanel.svelte:63-88`, `NewConversationButton.svelte:20`, `SettingsDialog.svelte:48`, `ModelSettings.svelte:155-161,266-273`, `GeneralSettings.svelte:42`, `AdvancedSettings.svelte` | retrait (erreurs, approbations, confiance conservées) ; rubrique Avancé masquée | relecture + capture |
| Typographie périphérique en 14 px | navigation, en-tête, paramètres, sélecteurs, titres de panneaux | 13 px pour contrôles/titres, 12 px conservé pour le secondaire ; `AssistantMessage` (14 px / 1.625), bulle utilisateur, saisie, terminal inchangés | capture native |

## 2. Changements effectués

**Opus (tous les changements visuels, MCP Svelte).**
- Séparations : `TopBar` `border-transparent` (hauteur inchangée) ; bordures des panneaux latéraux et du
  terminal retirées (`AppShell`), poignées de redimensionnement `bg-border/50` (zone de prise et clavier
  inchangés) ; pied « Paramètres » `border-sidebar-border/50`.
- Typographie périphérique : 13 px (`text-[13px]`) pour navigation, boutons « Nouvelle conversation »,
  « Nouveau projet », « Paramètres », recherche, titres de conversations/projets, en-tête, paramètres (onglets,
  titres, lignes), titres de panneaux, carte contexte, sélecteurs modèle/effort, bande projet, bouton « Plus » ;
  12 px pour les descriptions secondaires. Inchangés : `AssistantMessage` (14 px / 1.625), bulle utilisateur,
  saisie, terminal (13 px). Aucune classe globale ni composant shadcn modifié.
- Textes retirés : écran central (« Bienvenue… », « Écrivez un message… », chemin `providers.toml` → titre court
  + bouton « Ouvrir les paramètres »), timeline vide, navigation (« Aucun projet » + paragraphe, « Sélectionnez un
  projet… », « Aucune conversation » + consigne), infobulle fournisseur absent, en-tête des paramètres (gardé en
  `sr-only`), Modèles (aide « Cliquez… », « Fichier : », bloc « Choix par défaut / À venir »), Général
  (paragraphe d'effet), rubrique Avancé masquée (uniquement des fonctions indisponibles).
- Bilan d'exécution en millisecondes (`12 400 ms`) au lieu de secondes.
- Composeur : brouillon du projet quand aucune conversation n'est ouverte, création unique au premier envoi
  (`app.startConversation`), brouillon du projet vidé après un envoi accepté, focus sur « Nouvelle conversation »,
  `display` transmis pour `/skill`. Bulle utilisateur : étiquette monospace (`/skill wrangler`).

**Haiku 2 (logique TS).** `newConversation()` sans IPC ; brouillons et choix modèle/effort par projet ;
`startConversation` partagé entre appelants ; commandes moteur hors `submit` refusées sans conversation (aucune
création) ; étiquettes : optimistes au `run_started`, confirmées par `run_labeled`, restaurées par `user_index`
(repli texte pour les anciennes) ; messages `<skill_activation>` masqués à la relecture ; brouillon conservé sur
`lagged` ; une création tardive ne ramène pas la vue sur un autre projet.

**Haiku 1 (Rust).** `RunLabelStore` (`<bricks_home>/desktop/run-labels/<session>.jsonl`, append-only, dédup,
lignes abîmées tolérées, supprimé avec la session), corrélation des submits par ordre, `user_index` calculé sur
l'historique réel, `RunLabeled` publié en fin d'exécution ; correctif d'une course préexistante du journal des
modifications.

**Haiku 3 (tests).** 32 tests (flux de session, concurrence, étiquettes, persistance Rust) ; a identifié la fuite
de deux tests Rust vers le vrai `~/.bricks/sessions` (`engine/sessions.rs` ignorait le dossier de l'état) —
corrigée par l'orchestrateur — et trois défauts de logique corrigés par Haiku 2.

## 3. Preuves modèle / effort / MCP

Métadonnées d'exécution (pas d'auto-déclaration) : audit `claude-haiku-5-5` / `high` ; Haiku 1, 2, 3
`claude-haiku-5-5` / `xhigh` ; quatre sous-agents au total. MCP Svelte : Opus — `list-sections`,
`get-documentation` (`class`, `Global styles`), `svelte-autofixer` sur TopBar, AppShell, App, NavigationPanel,
SettingsDialog, ModelSettings, GeneralSettings, ConversationTimeline, Composer, UserMessage (aucun problème ;
suggestions préexistantes sur des `$effect`) ; Haiku 2 — appels constatés sur `app.svelte.ts` et
`conversation.svelte.ts`. Limite : l'effort de l'orchestrateur (`medium` demandé) ne peut pas être modifié depuis
la session.

## 4. Contrôles exécutés

- `npm run check` : 0 erreur. `npm run test:types` : 0. `npm run test:unit` : 38 fichiers, 570 tests.
- `cargo test -p bricks-desktop` : 164 + suites d'intégration (dont `run_labels_14_4` 8). `cargo test -p bricks-cli`
  ciblé : 5, 8, 5. `~/.bricks/sessions` : 178 entrées avant et après la suite (fuite corrigée).
- Build release : `Bricks.app` 36,38 Mo, aucun avertissement.
- **App native (premier plan, captures réelles)** :
  - bordure haute absente, séparations latérales discrètes, typographie 13 px, texte de l'agent inchangé ;
  - 5 + 3 clics « Nouvelle conversation » : liste inchangée, **0 session ajoutée sur disque** (174 → 174),
    composeur focalisé, modèle conservé ;
  - paramètres : sans phrase d'en-tête, sans rubrique Avancé, Général réduit au libellé + sélecteur, Modèles
    sans aide ni chemin ;
  - `/skill wrangler Réponds seulement OK.` : une seule conversation créée, bulle avec étiquette `/skill wrangler`,
    réponse réelle du fournisseur, étiquette restaurée après redémarrage.

- **Après la dernière compilation (autorisation Bureau accordée)** : historique rechargé sans le contenu
  d'activation du skill, étiquette restaurée ; second `/skill wrangler …` : étiquette visible dès le début de
  l'exécution (pendant « En cours… »), bilan « Terminé · 1 653 ms ».

- Brouillons (natif) : brouillon saisi dans « githubbricks », passage à « Desktop » (brouillon vide, son propre
  modèle DeepSeek/high), retour (texte et modèle Muse/medium conservés) ; ouverture d'une conversation existante
  (brouillon propre vide) puis « Nouvelle conversation » (brouillon du projet restauré) ; aucune session créée.

## 5. Défauts encore ouverts / non vérifiés

- Démarrage bloqué tant que macOS n'a pas autorisé l'accès au dossier Bureau (projet « Desktop ») : chaque nouvelle
  signature d'app redemande l'autorisation. Ce n'est pas un défaut du code (`projects_list` attend la réponse du
  système).
- Non vérifiés nativement : zoom et thème sombre.
- Les captures en arrière-plan d'une fenêtre WKWebView non redessinée sont trompeuses ; seules les captures au
  premier plan ont été retenues.
