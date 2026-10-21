<script lang="ts">
  // Hosts the inspector view opened by a `/` command (ui.inspector).
  // Owner: orchestrator. Each view lives in its owner's feature folder.
  import type { Component } from 'svelte'
  import * as Dialog from '$lib/components/ui/dialog'
  import { ui, type InspectorState, type InspectorView } from '$lib/state/ui.svelte'
  import HelpView from './HelpView.svelte'
  import SearchView from '$lib/features/inspect/SearchView.svelte'
  import MemoryView from '$lib/features/inspect/MemoryView.svelte'
  import ContextView from '$lib/features/inspect/ContextView.svelte'
  import CostView from '$lib/features/inspect/CostView.svelte'
  import ClearView from '$lib/features/inspect/ClearView.svelte'
  import AgentProfilesView from '$lib/features/catalog/AgentProfilesView.svelte'
  import SkillsView from '$lib/features/catalog/SkillsView.svelte'
  import JobsView from '$lib/features/catalog/JobsView.svelte'
  import ChangesetView from '$lib/features/catalog/ChangesetView.svelte'
  import SessionView from '$lib/features/session-info/SessionView.svelte'
  import SessionsView from '$lib/features/session-info/SessionsView.svelte'
  import ToolsView from '$lib/features/session-info/ToolsView.svelte'
  import McpView from '$lib/features/session-info/McpView.svelte'
  import ConfigView from '$lib/features/session-info/ConfigView.svelte'
  import FileView from '$lib/features/inspect/FileView.svelte'

  type ViewProps = { args: InspectorState['args'] }
  const VIEWS: Record<InspectorView, { title: string; component: Component<ViewProps> }> = {
    help: { title: 'Commandes', component: HelpView },
    search: { title: 'Recherche dans le code', component: SearchView },
    memory: { title: 'Mémoire', component: MemoryView },
    context: { title: 'Contexte', component: ContextView },
    cost: { title: 'Consommation', component: CostView },
    clear: { title: 'Vider le contexte', component: ClearView },
    agents: { title: 'Profils d’agents', component: AgentProfilesView },
    skills: { title: 'Skills', component: SkillsView },
    jobs: { title: 'Tâches de fond', component: JobsView },
    changeset: { title: 'Modifications isolées', component: ChangesetView },
    session: { title: 'Session', component: SessionView },
    sessions: { title: 'Reprendre une session', component: SessionsView },
    tools: { title: 'Outils', component: ToolsView },
    mcp: { title: 'Serveurs MCP', component: McpView },
    config: { title: 'Configuration', component: ConfigView },
    file: { title: 'Fichier du projet', component: FileView },
  }

  const entry = $derived(ui.inspector ? VIEWS[ui.inspector.view] : null)
</script>

<Dialog.Root
  open={ui.inspector !== null}
  onOpenChange={(open) => {
    if (!open) ui.close()
  }}
>
  <Dialog.Content class="flex max-h-[85vh] flex-col gap-3 sm:max-w-3xl">
    {#if entry && ui.inspector}
      <Dialog.Header>
        <Dialog.Title>{entry.title}</Dialog.Title>
      </Dialog.Header>
      <div class="min-h-0 flex-1 overflow-y-auto">
        <entry.component args={ui.inspector.args} />
      </div>
    {/if}
  </Dialog.Content>
</Dialog.Root>
