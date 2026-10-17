<script lang="ts">
	// Bottom pane: interactive shells on a real PTY, one set per project.
	// The sessions live in terminals.svelte.ts (store and session rules in
	// terminal-store / terminal-session); this component only shows the active
	// one.
	import '@xterm/xterm/css/xterm.css';
	import { untrack } from 'svelte';
	import Plus from '@lucide/svelte/icons/plus';
	import RotateCw from '@lucide/svelte/icons/rotate-cw';
	import X from '@lucide/svelte/icons/x';
	import { Button } from '$lib/components/ui/button';
	import EmptyState from '$lib/layout/EmptyState.svelte';
	import { cn } from '$lib/utils.js';
	import { app } from '$lib/state/app.svelte';
	import { theme } from '$lib/theme/theme.svelte';
	import { terminals } from './terminals.svelte';

	let container = $state<HTMLDivElement | null>(null);

	const projectId = $derived(app.currentProjectId);
	const list = $derived(projectId ? terminals.list(projectId) : []);
	const activeId = $derived(projectId ? terminals.activeId(projectId) : null);
	const active = $derived(list.find((s) => s.id === activeId) ?? null);
	const activeName = $derived(active ? `Terminal ${list.indexOf(active) + 1}` : 'Terminal');

	// The first terminal of a project opens by itself, once per application run.
	$effect(() => {
		const pid = projectId;
		if (!pid) return;
		untrack(() => terminals.seed(pid, app.current?.sessionId ?? null));
	});

	// Show the active terminal here, start its shell if needed, and keep its
	// size in step with the pane. The cleanup runs before the next terminal is
	// attached (tab switch, project switch) and when the pane closes: it takes
	// this terminal out of the container, without disposing it.
	$effect(() => {
		const session = active;
		const element = container;
		if (!session || !element) return;
		session.attach(element);
		untrack(() => {
			if (session.status === 'idle') session.start();
		});
		session.focus();
		const observer = new ResizeObserver(() => session.requestRefit());
		observer.observe(element);
		return () => {
			observer.disconnect();
			session.detach();
		};
	});

	// Terminal colours follow the theme. Read after the `.dark` class is set.
	$effect(() => {
		void theme.resolved;
		untrack(() => terminals.refreshTheme());
	});

	function select(id: string) {
		if (projectId) terminals.activate(projectId, id);
	}

	function close(id: string) {
		if (projectId) terminals.close(projectId, id);
	}

	function create() {
		if (projectId) terminals.create(projectId, app.current?.sessionId ?? null);
	}
</script>

<div class="flex h-full min-h-0 flex-col bg-code-bg">
	<div class="flex h-9 shrink-0 items-center gap-1 border-b border-border bg-background px-2">
		<div role="group" aria-label="Terminaux du projet" class="flex min-w-0 flex-1 items-center gap-1 overflow-x-auto">
			{#each list as session, index (session.id)}
				{@const isActive = session.id === activeId}
				<div class={cn('flex shrink-0 items-center rounded-md', isActive && 'bg-muted')}>
					<button
						type="button"
						aria-pressed={isActive}
						class={cn(
							'rounded-md px-2 py-1 text-xs outline-none focus-visible:ring-2 focus-visible:ring-ring',
							isActive ? 'text-foreground' : 'text-muted-foreground hover:text-foreground'
						)}
						onclick={() => select(session.id)}
					>
						Terminal {index + 1}
					</button>
					<button
						type="button"
						class="rounded-md p-1 text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
						aria-label={`Fermer le terminal ${index + 1}`}
						title="Fermer le terminal"
						onclick={() => close(session.id)}
					>
						<X class="size-3.5" aria-hidden="true" />
					</button>
				</div>
			{/each}
		</div>
		<Button
			variant="ghost"
			size="icon-sm"
			aria-label="Nouveau terminal"
			title="Nouveau terminal"
			disabled={!projectId}
			onclick={create}
		>
			<Plus class="size-4" aria-hidden="true" />
		</Button>
	</div>

	{#if active?.cwd}
		<p class="truncate px-3 pt-1 font-mono text-xs text-muted-foreground" title={active.cwd}>{active.cwd}</p>
	{/if}

	<div class="relative min-h-0 flex-1">
		<div bind:this={container} role="group" aria-label={activeName} class="absolute inset-0 px-2 py-1"></div>

		{#if !active}
			<div class="absolute inset-0 flex items-center justify-center bg-code-bg">
				<EmptyState
					title={projectId ? 'Aucun terminal ouvert' : 'Aucun projet ouvert'}
					description={projectId ? 'Ouvrez un terminal dans le dossier du projet.' : 'Choisissez un projet pour ouvrir un terminal.'}
				>
					{#snippet action()}
						{#if projectId}
							<Button variant="outline" size="sm" onclick={create}>Nouveau terminal</Button>
						{/if}
					{/snippet}
				</EmptyState>
			</div>
		{:else if active.status === 'exited'}
			<div
				role="status"
				class="absolute inset-x-0 bottom-0 flex items-center justify-between gap-2 border-t border-border bg-card px-3 py-2 text-xs text-card-foreground"
			>
				<span>
					Processus terminé{active.exitCode !== null ? ` (code ${active.exitCode})` : ''}
				</span>
				<Button variant="outline" size="sm" onclick={() => active?.restart()}>
					<RotateCw class="size-3.5" aria-hidden="true" />
					Relancer
				</Button>
			</div>
		{:else if active.status === 'error'}
			<div
				role="alert"
				class="absolute inset-x-0 bottom-0 flex items-center justify-between gap-2 border-t border-border bg-card px-3 py-2 text-xs text-destructive"
			>
				<span class="min-w-0 truncate">Impossible de démarrer le terminal : {active.error}</span>
				<Button variant="outline" size="sm" onclick={() => active?.restart()}>Réessayer</Button>
			</div>
		{/if}
	</div>
</div>
