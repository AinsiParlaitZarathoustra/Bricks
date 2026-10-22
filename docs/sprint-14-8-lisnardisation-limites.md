# Sprint 14.8 — Lisnardisation : limites des sous-agents

**Objectif : donner aux sous-agents des limites plus larges pour le résumé et la sortie d'outil, et borner la récursion à un niveau par défaut, sans abaisser les autres limites.**

**Statut : réglages et documentation modifiés, tests à relancer.** Base `9ac654b`, branche `lisnardize/agent-limits`, 22 octobre 2026.

## 1. Changements de réglages

| Réglage | Avant | Après |
| --- | --- | --- |
| `[agents] summary_chars` (résumé renvoyé au parent, en caractères Unicode) | 10 000 | 30 000 |
| `[agents] max_depth` (générations sous l'agent de session) | 2 | 1 |
| Sortie d'un outil dans un événement de sous-agent (`MAX_SUBAGENT_OUTPUT_BYTES`, non réglable) | 64 KiB | 96 KiB (98 304 octets) |

Avec `max_depth = 1`, l'agent de session lance des enfants, mais ceux-ci ne peuvent pas lancer de petits-enfants (`AgentDepthExceeded`). Un `max_depth = 2` explicite dans `bricks.toml` reste valable pour l'usage ADK.

Aucune autre limite n'est abaissée : 16 sous-agents simultanés, file de 64 entrées, admission de 60 s restent inchangés.

## 2. Fichiers modifiés dans le code

- `crates/bricks-agent/src/agents/spawn.rs` : valeurs par défaut `summary_chars` et `max_depth`.
- `crates/bricks-agent/src/control/protocol.rs` : `MAX_SUBAGENT_OUTPUT_BYTES` à 96 KiB.
- `crates/bricks-agent/src/bricks_config.rs` : le test de synchronisation de la documentation vérifie aussi `summary_chars`.
- `crates/bricks-agent/tests/agents.rs`, `tests/admission_window.rs`, `tests/agents_runtime.rs` : les tests de petits-enfants fixent `max_depth = 2` explicitement ; le test de refus de profondeur utilise le défaut.

## 3. Documentation modifiée

- `docs/agents.md` : exemple de configuration, paragraphe « Recursion and limits », enchaînement parent → enfant → petit-enfant, taille de sortie dans le schéma 6.
- `docs/cli.md` : taille de `output` dans les événements `agent_tool_finished`.
- `docs/content/docs/built-in-tools.mdx` : valeurs des limites.
- `docs/content/docs/migration.mdx` : taille de sortie, tableau des réglages (ligne `max_depth` ajoutée).
- `docs/content/docs/changelog.mdx` et `CHANGELOG.md` : entrée dans la section « Unreleased ».
- `apps/bricks-desktop/src/lib/ipc/engine.ts` : commentaire de `output`.

`docs/bricks.example.toml` était déjà à jour et n'a pas été modifié dans ce sprint.

## 4. Hors périmètre

L'audit reporte à des sprints ultérieurs :

- lecture paginée des fichiers ;
- indexation des gros fichiers ;
- embeddings ;
- relocation des sous-agents ;
- WebFetch ;
- workflows ;
- budget partagé entre sous-agents.

## Validation

Exécuté le 22 octobre 2026 sur la branche, sans appel fournisseur facturé :

- `cargo test -p bricks-agent --no-fail-fast` : **434 tests réussis**, 0 échec, 1 ignoré (`e2e_live_compression`, marqué `#[ignore]`).
- Ciblés : `--test agents` 14, `--test admission_window` 6, `--test agents_runtime` 37, `--test stop_contract --test edit_runtime` 18 + 9, `--lib bricks_config` 9, plus `a_sub_agent_output_is_cut_at_96_kib_on_a_character_boundary` et `a_cut_text_stays_on_a_character_boundary`.
- `cargo test -p bricks-desktop sub_agent` : 3 tests réussis.
- `cargo clippy -p bricks-agent --all-targets -- -D warnings` : aucun avertissement.
- `cargo fmt --check` : fichiers modifiés propres ; les écarts signalés dans `spawn.rs` et `agents_runtime.rs` existaient déjà sur `main`.

Non couvert : test natif de l'interface Desktop et run réel avec fournisseur.
