<script lang="ts">
	import { untrack } from 'svelte';
	import type { Conversation } from '$lib/state/conversation.svelte';
	import { Badge } from '$lib/components/ui/badge';
	import StatusDot from '$lib/layout/StatusDot.svelte';
	import AgentInspector from './AgentInspector.svelte';
	import {
		agentDurationMs,
		agentStateView,
		formatDuration,
		isLive,
		isolationLabel
	} from './agentView';

	interface Props {
		conversation: Conversation;
	}

	let { conversation }: Props = $props();

	/** Only the engine's real sub-agents of this conversation. */
	const agents = $derived([...conversation.agents.values()]);

	// One clock for the whole list, running only while an agent is live.
	let now = $state(Date.now());

	// Agent records are reactive (the conversation stores them as $state), so
	// the live check follows their state changes directly.
	const anyLive = $derived(agents.some((a) => isLive(a.state)));

	$effect(() => {
		if (!anyLive) return;
		untrack(() => (now = Date.now()));
		const id = setInterval(() => (now = Date.now()), 1000);
		return () => clearInterval(id);
	});

	let selectedId = $state<string | null>(null);
	const selected = $derived(selectedId ? (conversation.agents.get(selectedId) ?? null) : null);
</script>

{#if agents.length === 0}
	<p class="py-1 text-xs text-pretty text-muted-foreground">Aucun sous-agent dans cette conversation.</p>
{:else}
	<ul class="flex flex-col gap-2">
		{#each agents as agent (agent.id)}
			{@const view = agentStateView(agent.state)}
			<li>
				<button
					type="button"
					onclick={() => (selectedId = agent.id)}
					class="flex w-full min-w-0 flex-col gap-1.5 rounded-md border border-border bg-background px-2.5 py-2 text-left outline-none hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring"
				>
					<span class="flex min-w-0 items-center gap-2">
						<span class="min-w-0 flex-1 truncate text-sm font-medium">{agent.spawn.profile}</span>
						<StatusDot status={view.dot} label={view.label} showLabel class="shrink-0" />
					</span>
					<span class="line-clamp-2 text-xs text-pretty text-muted-foreground">
						{agent.spawn.description || agent.spawn.task}
					</span>
					<span class="flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted-foreground">
						<span class="font-mono" title="Modèle appliqué">{agent.spawn.model.applied}</span>
						{#if agent.spawn.model.requested && agent.spawn.model.requested !== agent.spawn.model.applied}
							<span title={agent.spawn.model.reason ?? undefined}>
								(demandé : {agent.spawn.model.requested})
							</span>
						{/if}
						<span title="Raisonnement appliqué">{agent.spawn.reasoning.applied || 'par défaut'}</span>
						{#if agent.spawn.reasoning.requested && agent.spawn.reasoning.requested !== agent.spawn.reasoning.applied}
							<span title={agent.spawn.reasoning.reason ?? undefined}>
								(demandé : {agent.spawn.reasoning.requested})
							</span>
						{/if}
						<span class="tabular-nums">{formatDuration(agentDurationMs(agent, now))}</span>
						<Badge
							variant="outline"
							class="h-5"
							title={agent.spawn.branch ? `Branche ${agent.spawn.branch}` : undefined}
						>
							{isolationLabel(agent.spawn.isolation)}
						</Badge>
					</span>
				</button>
			</li>
		{/each}
	</ul>
{/if}

{#if selected}
	<AgentInspector agent={selected} {conversation} {now} onClose={() => (selectedId = null)} />
{/if}
