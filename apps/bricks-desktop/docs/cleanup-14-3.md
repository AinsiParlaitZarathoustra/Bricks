# Sprint 14.3 — nettoyage du code Rust interne

Rapport de l'agent R1 (Haiku 5.5), retranscrit par l'orchestrateur. API publiques du SDK toutes conservées.

## Supprimé
- `cersei-agent/tests/agents_runtime.rs` : helper `_msg` inutilisé et l'import `Message` qu'il seul utilisait.
- `cersei-types/src/lib.rs` : deux `#[allow(unused)]` sans effet sur les champs de `StreamEvent::ContentBlockStart`.
- `#![allow(dead_code)]` de `cersei-agent/tests/common/mod.rs` et `cersei-provider/tests/common/mod.rs` (aucun avertissement sans).
- `#[allow(dead_code)]` sur `struct Agent` remplacé par une annotation au seul champ concerné.

## Mutualisé
- `cersei-tools/src/file_history.rs` : `record_read/write/edit` partagent un helper privé `record` (−16 lignes, tests existants).
- `cersei-tools/src/git_utils.rs` : dix blocs `Command::new("git")` passent par `run_git` / `git_stdout` /
  `git_stdout_or_empty`, signatures et règles de statut inchangées ; nouveau test sur dépôt temporaire.

## Annotations resserrées
- `cersei-tools/src/tasks.rs` : allow de structure ramené au champ `prompt`.
- `cersei-agent/src/lib.rs` : allow au champ `append_system_prompt`, avec sa raison.
- Total : 9 attributs `allow` avant, 4 après, tous justifiés.

## Conservé (compatibilité)
`file_snapshot`, `file_watcher` (deux variantes cfg), `vm_tools` (feature `vms`), `lsp_tool`,
`tool_primitives::code_intel`, `auto_dream`, `coordinator`, `session_memory` (exemples `crates/cersei/examples`),
symboles publics isolés de `compact`, LSP, MCP, provider, mémoire (analyse dead-code propre sur toutes cibles et
features), `AgentControl::PermissionResponse` / `InjectMessage` (construits par des méthodes publiques).

## Bilan
9 fichiers, +97 / −116 lignes, **net −19** sur le code suivi.

## Contrôles (rapportés par R1)
Build `--all-features` de cersei-types, cersei-tools, cersei-provider, cersei-agent ; `--examples` de cersei-tools
et cersei-agent ; tests lib et intégration : cersei-types 29, cersei-tools 194 + 23, cersei-provider 142 + 18 + 1,
cersei-agent 178 lib / 360 au total ; clippy `--all-targets --all-features` : 0 avertissement avant et après.

## Défauts préexistants relevés, non corrigés (changement de comportement)
1. `AgentBuilder::append_system_prompt` est sans effet (`effective_system()` ne lit pas le champ).
2. `AgentStream::respond_permission` et `inject_message` sont acceptés puis ignorés (limitation documentée,
   `wiki/05-events-streaming.md`).
3. L'entrée `prompt` de TaskCreate figure au schéma mais est ignorée.
