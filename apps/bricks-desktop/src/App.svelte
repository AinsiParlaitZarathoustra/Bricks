<script lang="ts">
  // Root: wires the shell, the global state and the feature entry points.
  // Owner: orchestrator.
  import { onMount } from 'svelte'
  import { toast } from 'svelte-sonner'
  import { Toaster } from '$lib/components/ui/sonner'
  import * as Tooltip from '$lib/components/ui/tooltip'
  import AppShell from '$lib/layout/AppShell.svelte'
  import TopBar from '$lib/layout/TopBar.svelte'
  import EmptyState from '$lib/layout/EmptyState.svelte'
  import type { ShellLayout } from '$lib/layout/types'
  import { initTheme, setTheme } from '$lib/theme/theme.svelte'
  import { app } from '$lib/state/app.svelte'
  import NavigationPanel from '$lib/features/navigation/NavigationPanel.svelte'
  import ConversationView from '$lib/features/conversation/ConversationView.svelte'
  import ConversationTitle from '$lib/features/conversation/ConversationTitle.svelte'
  import Composer from '$lib/features/composer/Composer.svelte'
  import ContextPanel from '$lib/features/context/ContextPanel.svelte'
  import ChangesPanel from '$lib/features/changes/ChangesPanel.svelte'
  import TerminalPanel from '$lib/features/terminal/TerminalPanel.svelte'
  import SettingsDialog from '$lib/features/settings/SettingsDialog.svelte'
  import InspectorHost from '$lib/features/commands/InspectorHost.svelte'
  import { ui } from '$lib/state/ui.svelte'
  import { Button } from '$lib/components/ui/button'


  let leftOpen = $state(true)
  let rightOpen = $state(true)
  let terminalOpen = $state(false)
  let leftWidth = $state(260)
  let rightWidth = $state(280)
  // Terminal share of the centre column. `legacyTerminalHeight` is the
  // saved pixel height, read by AppShell once to derive the ratio when none is saved.
  let terminalRatio = $state<number | null>(null)
  let legacyTerminalHeight = $state<number | undefined>(undefined)

  onMount(() => {
    const disposeTheme = initTheme('system')
    app.init().then(() => {
      const p = app.prefs
      setTheme(p.theme)
      leftOpen = p.left_open
      rightOpen = p.right_open
      terminalOpen = p.terminal_open
      leftWidth = p.left_width
      rightWidth = p.right_width
      terminalRatio = p.terminal_ratio
      legacyTerminalHeight = p.terminal_height
    })
    return () => {
      disposeTheme()
      app.dispose()
    }
  })

  $effect(() => {
    const err = app.lastError
    if (err) toast.error(err.message)
  })

  function onLayoutChange(l: ShellLayout) {
    app.setPrefs({
      left_open: l.leftOpen,
      right_open: l.rightOpen,
      terminal_open: l.terminalOpen,
      left_width: Math.round(l.leftWidth),
      right_width: Math.round(l.rightWidth),
      terminal_ratio: l.terminalRatio,
    })
  }

  // `/diff` (or any caller) sets app.rightView = 'changes': show the panel.
  $effect(() => {
    if (app.rightView === 'changes') rightOpen = true
  })

  function toggleChanges() {
    if (app.rightView === 'changes' && rightOpen) {
      app.rightView = 'context'
    } else {
      app.rightView = 'changes'
      rightOpen = true
    }
  }
</script>

<Tooltip.Provider>
  {#if !app.ready}
    <div class="grid h-screen place-items-center text-sm text-muted-foreground">Démarrage de Bricks…</div>
  {:else if app.startError}
    <div class="grid h-screen place-items-center p-6">
      <EmptyState title="Bricks n’a pas pu démarrer" description={app.startError} />
    </div>
  {:else}
    <AppShell
      bind:leftOpen
      bind:rightOpen
      bind:terminalOpen
      bind:leftWidth
      bind:rightWidth
      bind:terminalRatio
      {legacyTerminalHeight}
      {onLayoutChange}
    >
      {#snippet topbar(controls)}
        <TopBar {...controls} changesPressed={app.rightView === 'changes' && rightOpen} onToggleChanges={toggleChanges}>
          {#snippet title()}
            <ConversationTitle conversation={app.current} />
          {/snippet}
        </TopBar>
      {/snippet}
      {#snippet left()}
        <NavigationPanel />
      {/snippet}
      {#snippet center()}
        <div class="flex h-full min-h-0 flex-col">
          <div class="min-h-0 flex-1">
            {#if app.current}
              {#key app.current.sessionId}
                <ConversationView conversation={app.current} />
              {/key}
            {:else}
              <div class="grid h-full place-items-center p-6">
                {#if !app.hasProviders}
                  <EmptyState title="Aucun fournisseur configuré">
                    {#snippet action()}
                      <Button variant="ghost" size="sm" onclick={() => (ui.settingsOpen = true)}>Ouvrir les paramètres</Button>
                    {/snippet}
                  </EmptyState>
                {/if}
              </div>
            {/if}
          </div>
          <Composer conversation={app.current} />
        </div>
      {/snippet}
      {#snippet right()}
        {#if app.rightView === 'changes'}
          <ChangesPanel />
        {:else}
          <ContextPanel />
        {/if}
      {/snippet}
      {#snippet terminal(hide)}
        <TerminalPanel onHide={hide} />
      {/snippet}
    </AppShell>
  {/if}
  <SettingsDialog bind:open={ui.settingsOpen} />
  <InspectorHost />
  <Toaster richColors />
</Tooltip.Provider>
