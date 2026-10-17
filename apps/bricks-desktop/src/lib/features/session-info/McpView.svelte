<script lang="ts">
	// MCP servers of the session (`/mcp`): name and state as the engine reports
	// them. Read-only: no installation or management here. Owner: Haiku 6 (Sprint 14.1).
	import { untrack } from 'svelte';
	import { Badge } from '$lib/components/ui/badge';
	import { errorMessage, sessionMcp, type McpServerInfo } from '$lib/ipc/client';
	import EmptyState from '$lib/layout/EmptyState.svelte';
	import { app } from '$lib/state/app.svelte';
	import type { InspectorState } from '$lib/state/ui.svelte';
	import { mcpStatusView } from './format';

	// Opened without options: the view shows the conversation on screen.
	let { args: _args }: { args: InspectorState['args'] } = $props();

	const conv = $derived(app.current);
	const sessionId = $derived(conv?.sessionId ?? null);

	let servers = $state<McpServerInfo[] | null>(null);
	let loadError = $state<string | null>(null);

	/** Only the newest read may write the view (a slower earlier one is dropped). */
	let loadToken = 0;

	async function load(id: string) {
		const token = ++loadToken;
		try {
			const next = await sessionMcp(id);
			if (token !== loadToken) return;
			servers = next;
			loadError = null;
		} catch (e) {
			if (token !== loadToken) return;
			loadError = errorMessage(e);
		}
	}

	// Read on open, and again when a run starts or ends: servers connect at the
	// first run of the session.
	$effect(() => {
		const id = sessionId;
		void conv?.running;
		if (!id) {
			loadToken++;
			servers = null;
			loadError = null;
			return;
		}
		untrack(() => void load(id));
	});
</script>

{#if !conv || !sessionId}
	<EmptyState
		title="Aucune conversation ouverte"
	/>
{:else if loadError}
	<p role="alert" class="text-sm text-destructive">Lecture des serveurs MCP impossible : {loadError}</p>
{:else if servers === null}
	<p class="text-sm text-muted-foreground" aria-live="polite">Chargement des serveurs MCP…</p>
{:else if servers.length === 0}
	<EmptyState title="Aucun serveur MCP pour ce projet" />
{:else}
	<ul class="flex flex-col divide-y divide-border rounded-lg border border-border">
		{#each servers as server, i (i)}
			{@const view = mcpStatusView(server.status)}
			<li class="flex min-w-0 flex-col gap-1 px-3 py-2.5 sm:flex-row sm:items-center sm:justify-between sm:gap-4">
				<span class="min-w-0 font-mono text-sm break-all">{server.name}</span>
				<span class="flex min-w-0 flex-wrap items-center gap-2 sm:justify-end">
					<Badge
						variant={view.tone === 'error' ? 'destructive' : view.tone === 'success' ? 'secondary' : 'outline'}
					>
						{view.label}
					</Badge>
					{#if view.detail}
						<span class="min-w-0 text-xs break-words text-muted-foreground">{view.detail}</span>
					{/if}
				</span>
			</li>
		{/each}
	</ul>
{/if}
