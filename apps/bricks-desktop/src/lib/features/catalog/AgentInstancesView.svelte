<script lang="ts">
	import { onMount, untrack } from 'svelte';
	import * as AlertDialog from '$lib/components/ui/alert-dialog';
	import { Button } from '$lib/components/ui/button';
	import AgentInspector from '$lib/features/agents/AgentInspector.svelte';
	import AgentList from '$lib/features/agents/AgentList.svelte';
	import {
		agentDurationMs,
		agentStateView,
		canCancel,
		controlDoneLabel,
		controlFailedLabel,
		formatDuration,
		formatTime,
		isLive,
		isolationLabel,
		terminationLabel
	} from '$lib/features/agents/agentView';
	import StatusDot from '$lib/layout/StatusDot.svelte';
	import { app } from '$lib/state/app.svelte';
	import type { InspectorState } from '$lib/state/ui.svelte';
	import { argText } from './labels';
	import { EngineRequest, controlSlot, type ControlAnswer } from './request.svelte';

	let { args }: { args: InspectorState['args'] } = $props();

	const conv = $derived(app.current);
	const agentId = $derived(argText(args.agentId));
	/** What the caller asked: `status`, `result`, `cancel`, or nothing. */
	const requested = $derived(argText(args.action));
	const agent = $derived(conv && agentId ? (conv.agents.get(agentId) ?? null) : null);
	const changesetCount = $derived(
		conv && agentId ? [...conv.changesets.values()].filter((c) => c.agent_id === agentId).length : 0
	);

	/** Show every agent instead of the unknown id. */
	let showAll = $state(false);
	/** The full AgentInspector of the agent, opened from the summary. */
	let detail = $state(false);
	let confirmOpen = $state(false);

	const request = new EngineRequest();
	const answer = $derived(request.answered<ControlAnswer>());

	// One clock for the summary, running only while the agent is live.
	let now = $state(Date.now());
	const live = $derived(agent ? isLive(agent.state) : false);

	$effect(() => {
		if (!live) return;
		untrack(() => (now = Date.now()));
		const id = setInterval(() => (now = Date.now()), 1000);
		return () => clearInterval(id);
	});

	function stopConfirmed() {
		if (agentId) {
			void request.send(
				{ type: 'agent_control', action: 'cancel', agent_id: agentId },
				controlSlot,
				'cancel'
			);
		}
	}

	onMount(() => {
		const id = agentId;
		const target = agent;
		if (!id || !target || !requested) return;
		if (requested === 'status' || requested === 'result') {
			void request.send(
				{ type: 'agent_control', action: requested, agent_id: id },
				controlSlot,
				requested
			);
		} else if (requested === 'cancel' && canCancel(target.state)) {
			confirmOpen = true;
		}
	});
</script>

{#if !conv}
	<p class="text-sm text-muted-foreground">Aucune conversation ouverte.</p>
{:else if agentId && !showAll}
	{#if !agent}
		<div class="flex flex-col items-start gap-3 text-sm">
			<p role="alert" class="rounded-md bg-destructive/10 px-3 py-2 text-xs text-pretty text-destructive">
				Aucun sous-agent « {agentId} » dans cette conversation.
			</p>
			<Button variant="outline" size="sm" onclick={() => (showAll = true)}>Voir les sous-agents</Button>
		</div>
	{:else if detail}
		<AgentInspector agent={agent} conversation={conv} {now} onClose={() => (detail = false)} />
	{:else}
		{@const view = agentStateView(agent.state)}
		<div class="flex flex-col gap-4 text-sm">
			<div class="flex min-w-0 items-center gap-2">
				<span class="min-w-0 truncate font-mono font-medium">{agent.spawn.profile}</span>
				<StatusDot status={view.dot} label={view.label} showLabel class="shrink-0" />
			</div>
			<p class="text-pretty whitespace-pre-wrap">{agent.spawn.description || agent.spawn.task}</p>

			<dl class="grid grid-cols-[7rem_minmax(0,1fr)] gap-x-3 gap-y-1.5 text-xs">
				<dt class="text-muted-foreground">Identifiant</dt>
				<dd class="font-mono break-all">{agent.id}</dd>
				<dt class="text-muted-foreground">Démarré</dt>
				<dd class="tabular-nums">{formatTime(agent.spawnedAt)}</dd>
				<dt class="text-muted-foreground">Durée</dt>
				<dd class="tabular-nums">
					{formatDuration(agentDurationMs(agent, now))}{live ? ' (en cours)' : ''}
				</dd>
				<dt class="text-muted-foreground">Modèle</dt>
				<dd class="font-mono break-all">{agent.spawn.model.applied}</dd>
				<dt class="text-muted-foreground">Isolation</dt>
				<dd>{isolationLabel(agent.spawn.isolation)}</dd>
				<dt class="text-muted-foreground">Appels d’outils</dt>
				<dd class="tabular-nums">{agent.tools.length}</dd>
				<dt class="text-muted-foreground">Modifications isolées</dt>
				<dd class="tabular-nums">{changesetCount}</dd>
			</dl>

			{#if agent.result}
				<section aria-label="Résultat" class="flex flex-col gap-1">
					<h3 class="text-xs font-medium text-muted-foreground">Résultat</h3>
					<p class="text-pretty whitespace-pre-wrap">{agent.result.summary || 'Aucun résumé.'}</p>
					{#if terminationLabel(agent.result.termination)}
						<p class="text-xs text-muted-foreground">{terminationLabel(agent.result.termination)}</p>
					{/if}
				</section>
			{/if}

			{#if request.refusal}
				<p role="alert" class="rounded-md bg-destructive/10 px-3 py-2 text-xs text-pretty text-destructive">
					Commande refusée : {request.refusal}
				</p>
			{:else if request.pending}
				<p role="status" class="text-xs text-muted-foreground">Réponse du moteur en attente…</p>
			{:else if answer}
				{#if answer.ok}
					<section aria-label="Réponse du moteur" class="flex flex-col gap-1">
						<h3 class="text-xs font-medium text-muted-foreground">
							{answer.action === 'cancel' ? controlDoneLabel(answer.action) : 'Réponse du moteur'}
						</h3>
						<pre class="max-h-64 overflow-auto rounded-md bg-code-bg px-2.5 py-2 font-mono text-xs whitespace-pre-wrap text-code-foreground">{answer.text}</pre>
					</section>
				{:else}
					<p role="alert" class="rounded-md bg-destructive/10 px-3 py-2 text-xs text-pretty text-destructive">
						{controlFailedLabel(answer.action)} : {answer.text}
					</p>
				{/if}
			{/if}

			{#if requested === 'cancel' && !canCancel(agent.state)}
				<p class="text-xs text-pretty text-muted-foreground">
					Ce sous-agent ne peut pas être arrêté (état : {view.label.toLowerCase()}).
				</p>
			{/if}

			<div class="flex flex-wrap gap-2">
				{#if canCancel(agent.state)}
					<Button variant="outline" size="sm" disabled={request.pending} onclick={() => (confirmOpen = true)}>
						Arrêter l’agent
					</Button>
				{/if}
				<Button variant="outline" size="sm" onclick={() => (detail = true)}>Détails complets</Button>
			</div>
		</div>

		<AlertDialog.Root bind:open={confirmOpen}>
			<AlertDialog.Content>
				<AlertDialog.Header>
					<AlertDialog.Title>Arrêter ce sous-agent ?</AlertDialog.Title>
					<AlertDialog.Description>« {agent.spawn.profile} » sera arrêté.</AlertDialog.Description>
				</AlertDialog.Header>
				<AlertDialog.Footer>
					<AlertDialog.Cancel>Annuler</AlertDialog.Cancel>
					<AlertDialog.Action variant="destructive" onclick={stopConfirmed}>Arrêter l’agent</AlertDialog.Action>
				</AlertDialog.Footer>
			</AlertDialog.Content>
		</AlertDialog.Root>
	{/if}
{:else}
	{#if conv.agents.size === 0}
		<p class="text-sm text-muted-foreground">Aucun sous-agent dans cette conversation.</p>
	{:else}
		<AgentList conversation={conv} />
	{/if}
{/if}
