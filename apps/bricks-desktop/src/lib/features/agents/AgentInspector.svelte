<script lang="ts">
	import FileIcon from '$lib/features/file-icons/FileIcon.svelte';
	import { app } from '$lib/state/app.svelte';
	import type { AgentInfo, Conversation } from '$lib/state/conversation.svelte';
	import * as AlertDialog from '$lib/components/ui/alert-dialog';
	import * as Dialog from '$lib/components/ui/dialog';
	import { Button } from '$lib/components/ui/button';
	import { Badge } from '$lib/components/ui/badge';
	import DiffStat from '$lib/layout/DiffStat.svelte';
	import StatusDot from '$lib/layout/StatusDot.svelte';
	import * as Collapsible from '$lib/components/ui/collapsible';
	import { Textarea } from '$lib/components/ui/textarea';
	import ChevronDown from '@lucide/svelte/icons/chevron-down';
	import type { ChangedFile, Choice } from '$lib/ipc/engine';
	import {
		agentDurationMs,
		agentStateView,
		canCancel,
		changeStatusLabel,
		changesetLabel,
		changesetVariant,
		controlDoneLabel,
		controlFailedLabel,
		formatCount,
		formatDuration,
		formatTime,
		isLive,
		isolationLabel,
		plural,
		terminationLabel
	} from './agentView';
	import ToolLine from '$lib/features/conversation/timeline/ToolLine.svelte';
	import MarkdownView from '$lib/features/conversation/markdown/MarkdownView.svelte';
	import { revealAgentFile } from '$lib/ipc/client';
	import { toolLineState } from '$lib/features/conversation/timeline/toolLine';

	interface Props {
		agent: AgentInfo;
		conversation: Conversation;
		/** Shared clock of the list, in ms. */
		now: number;
		onClose: () => void;
	}

	let { agent, conversation, now, onClose }: Props = $props();

	const view = $derived(agentStateView(agent.state));
	const live = $derived(isLive(agent.state));
	const cancellable = $derived(canCancel(agent.state));
	const durationMs = $derived(agentDurationMs(agent, now));
	const spawn = $derived(agent.spawn);
	/** The agent ended by cancellation or interruption: its unfinished tool calls are interruptions. */
	const agentInterrupted = $derived(agent.state === 'cancelled' || agent.state === 'interrupted');
	const result = $derived(agent.result);
	const termination = $derived(terminationLabel(result?.termination));
	const changesets = $derived(
		[...conversation.changesets.values()].filter((c) => c.agent_id === agent.id)
	);

	type ControlAction = 'cancel' | 'message' | 'apply_changes' | 'discard_changes';
	type ChangesetAction = Exclude<ControlAction, 'cancel' | 'message'>;

	/** Control action sent from here, and its refusal (if the app refused it). */
	let busy = $state<ControlAction | null>(null);
	let feedback = $state<string | null>(null);
	/**
	 * Engine answer seen before the last command sent from here. The answer to
	 * that command is the first one that differs from it.
	 */
	let baseline = $state.raw<Conversation['lastAgentControl'] | undefined>(undefined);
	const answer = $derived(
		baseline !== undefined && conversation.lastAgentControl !== baseline
			? conversation.lastAgentControl
			: null
	);
	/** Waiting for the engine's answer: the buttons stay disabled until it arrives. */
	const waiting = $derived(busy !== null && answer === null);

	async function control(
		action: ControlAction,
		target: { agent_id?: string; changeset_id?: string; message?: string }
	) {
		busy = action;
		feedback = null;
		baseline = conversation.lastAgentControl;
		const refusal = await app.command(conversation.sessionId, {
			type: 'agent_control',
			action,
			...target
		});
		if (refusal) {
			busy = null;
			feedback = refusal;
		}
	}

	// Apply and discard write to, or drop from, the project: confirm first.
	let confirmOpen = $state(false);
	let confirmTarget = $state<{ action: ChangesetAction; changesetId: string } | null>(null);
	const confirmChangeset = $derived(
		confirmTarget ? (conversation.changesets.get(confirmTarget.changesetId) ?? null) : null
	);

	function askChangeset(action: ChangesetAction, changesetId: string) {
		confirmTarget = { action, changesetId };
		confirmOpen = true;
	}

	function confirmChangesetAction() {
		const target = confirmTarget;
		confirmOpen = false;
		if (target) void control(target.action, { changeset_id: target.changesetId });
	}

	// Stopping an agent is final: confirm first, then send the cancel.
		let cancelOpen = $state(false);

		/** A message for the running agent: read at its next step (the engine says if not). */
		let message = $state('');
		function sendMessage(event: SubmitEvent) {
			event.preventDefault();
			const text = message.trim();
			if (text === '' || waiting) return;
			message = '';
			void control('message', { agent_id: agent.id, message: text });
		}

		function confirmCancel() {
			cancelOpen = false;
			void control('cancel', { agent_id: agent.id });
		}

		function choiceNote(choice: Choice): string | null {
		if (!choice.requested || choice.requested === choice.applied) return null;
		return `demandé : ${choice.requested}${choice.reason ? ` (${choice.reason})` : ''}`;
	}

	// Ids of the rendered task and result: unique per agent and zone, never a
	// prefix of another (length in front), so no anchor collides with the chat.
	const safeId = $derived(agent.id.toLowerCase().replace(/[^a-z0-9-]/g, '-'));
	const taskPrefix = $derived(`a${safeId.length}-${safeId}-task`);
	const resultPrefix = $derived(`a${safeId.length}-${safeId}-result`);

	/** A changed file as shown: its name; the path relative to the agent's workspace as a hint. */
	function fileName(path: string): string {
		return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
	}
	function relativePath(path: string): string {
		const root = spawn.workspace.replace(/[\\/]+$/, '');
		return path.startsWith(root + '/') ? path.slice(root.length + 1) : path;
	}

	/** Error of the last file that could not be revealed in the Finder. */
	let revealError = $state<string | null>(null);
	async function reveal(path: string) {
		revealError = null;
		try {
			await revealAgentFile(conversation.sessionId, agent.id, path);
		} catch (e) {
			revealError = e instanceof Error ? e.message : String(e);
		}
	}

	const modelNote = $derived(choiceNote(spawn.model));
	const reasoningNote = $derived(choiceNote(spawn.reasoning));
</script>

{#snippet fileRows(files: ChangedFile[])}
	<ul class="flex flex-col gap-1 text-xs">
		{#each files as file (file.path)}
			<li class="flex min-w-0 items-center gap-2">
				<span class="w-16 shrink-0 text-muted-foreground">{changeStatusLabel(file.status)}</span>
				<span class="flex min-w-0 flex-1 items-start gap-1.5 font-mono">
					<span class="flex h-4 shrink-0 items-center"><FileIcon path={file.path} /></span>
					<span class="min-w-0 break-all">
						{file.path}
						{#if file.from}<span class="text-muted-foreground">← {file.from}</span>{/if}
					</span>
				</span>
				{#if file.binary}
					<span class="shrink-0 text-muted-foreground">binaire</span>
				{:else}
					<DiffStat additions={file.added ?? 0} deletions={file.removed ?? 0} class="shrink-0" />
				{/if}
			</li>
		{/each}
	</ul>
{/snippet}

<Dialog.Root open onOpenChange={(open) => { if (!open) onClose(); }}>
	<Dialog.Content class="gap-4 sm:max-w-2xl">
		<Dialog.Header>
			<Dialog.Title class="flex min-w-0 items-center gap-2 pr-8">
				<span class="min-w-0 truncate">{spawn.profile}</span>
				<StatusDot status={view.dot} label={view.label} showLabel class="shrink-0" />
			</Dialog.Title>
			{#if spawn.description}
				<Dialog.Description class="text-pretty">{spawn.description}</Dialog.Description>
			{:else}
				<Dialog.Description class="sr-only">Détails du sous-agent</Dialog.Description>
			{/if}
		</Dialog.Header>

		<!-- Scrollable region: it must be keyboard-focusable (WAI). -->
		<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
		<div
			role="region"
			aria-label="Détails du sous-agent"
			tabindex="0"
			class="flex max-h-[62vh] flex-col gap-5 overflow-y-auto rounded-sm pr-2 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
		>
			{#if cancellable}
				<div>
					<Button
						variant="outline"
						size="sm"
						disabled={waiting}
						onclick={() => (cancelOpen = true)}
					>
						{waiting && busy === 'cancel' ? 'Arrêt…' : 'Arrêter l’agent'}
					</Button>
				</div>
			{/if}

			{#if feedback}
				<p role="alert" class="rounded-md bg-destructive/10 px-3 py-2 text-xs text-pretty text-destructive">
					{feedback}
				</p>
			{/if}

			{#if answer}
				{#if answer.ok}
					<p role="status" class="text-xs text-pretty text-muted-foreground">
						{controlDoneLabel(answer.action)}
					</p>
				{:else}
					<p role="alert" class="rounded-md bg-destructive/10 px-3 py-2 text-xs text-pretty text-destructive">
						{controlFailedLabel(answer.action)} : {answer.text}
					</p>
				{/if}
			{/if}

			{#if live && agent.state !== 'cancelling'}
				<form class="flex flex-col gap-2" onsubmit={sendMessage}>
					<label for={`${safeId}-message`} class="text-xs font-medium text-muted-foreground">Message à l’agent</label>
					<div class="flex items-end gap-2">
						<Textarea
							id={`${safeId}-message`}
							bind:value={message}
							rows={2}
							maxlength={16000}
							class="min-h-0 flex-1 resize-none text-sm"
						/>
						<Button type="submit" size="sm" disabled={waiting || message.trim() === ''}>
							{waiting && busy === 'message' ? 'Envoi…' : 'Envoyer'}
						</Button>
					</div>
				</form>
			{/if}

			<section class="flex flex-col gap-2" aria-labelledby={`${safeId}-result`}>
				<h3 id={`${safeId}-result`} class="text-xs font-medium text-muted-foreground">Résultat</h3>
				{#if !result}
					<p class="text-xs text-pretty text-muted-foreground">
						{live
							? 'Le résultat sera disponible à la fin du sous-agent.'
							: 'Aucun résultat enregistré.'}
					</p>
				{:else}
					{#if result.status !== 'completed'}
						<p class="text-xs text-pretty {result.status === 'cancelled' ? 'text-muted-foreground' : 'text-destructive'}">
							{view.label}{result.error ? ` : ${result.error}` : ''}
						</p>
					{/if}
					{#if termination}
						<p class="text-xs text-muted-foreground">{termination}</p>
					{/if}
					{#if result.summary}
						<div class="flex flex-col gap-1">
							<MarkdownView source={result.summary} idPrefix={resultPrefix} />
							{#if result.summary_truncated}
								<p class="text-xs text-muted-foreground">Résumé tronqué.</p>
							{/if}
						</div>
					{:else if result.status === 'completed'}
						<p class="text-xs text-muted-foreground">Réponse vide.</p>
					{/if}
					{#if result.warnings.length > 0}
						<ul class="flex list-disc flex-col gap-0.5 pl-4 text-xs text-pretty text-warning">
							{#each result.warnings as warning, i (i)}
								<li>{warning}</li>
							{/each}
						</ul>
					{/if}
				{/if}
			</section>

			<section class="flex flex-col gap-2" aria-labelledby={`${safeId}-files`}>
				<h3 id={`${safeId}-files`} class="text-xs font-medium text-muted-foreground">Fichiers modifiés</h3>
				{#if result && result.files_changed.length > 0}
					<ul class="flex flex-col gap-0.5 text-xs">
						{#each result.files_changed as path (path)}
							<li class="min-w-0">
								<button
									type="button"
									class="inline-flex max-w-full items-center gap-1.5 rounded-sm text-left font-mono text-primary hover:opacity-80 focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
									title={relativePath(path)}
									onclick={() => reveal(path)}
								>
									<FileIcon {path} />
									<span class="min-w-0 truncate underline decoration-dotted underline-offset-2">{fileName(path)}</span>
								</button>
							</li>
						{/each}
					</ul>
					{#if revealError}
						<p role="alert" class="text-xs text-pretty text-destructive">{revealError}</p>
					{/if}
				{:else if changesets.length === 0}
					<p class="text-xs text-muted-foreground">Aucun fichier modifié.</p>
				{/if}
				{#each changesets as cs (cs.id)}
					<div class="flex flex-col gap-2 rounded-md border border-border p-2.5">
						<div class="flex flex-wrap items-center gap-2 text-xs">
							<Badge variant={changesetVariant(cs.state)}>{changesetLabel(cs.state)}</Badge>
							<span class="text-muted-foreground">{plural(cs.files.length, 'fichier', 'fichiers')}</span>
							<span class="font-mono text-xs break-all text-muted-foreground">{cs.branch}</span>
						</div>
						{#if cs.note}
							<p class="text-xs text-pretty text-muted-foreground">{cs.note}</p>
						{/if}
						{#if cs.files.length > 0}
							{@render fileRows(cs.files)}
						{/if}
						{#if cs.state === 'ready'}
							<div class="flex flex-wrap gap-2 pt-1">
								<Button size="sm" disabled={waiting} onclick={() => askChangeset('apply_changes', cs.id)}>
									{waiting && busy === 'apply_changes' ? 'Application…' : 'Appliquer'}
								</Button>
								<Button
									variant="outline"
									size="sm"
									disabled={waiting}
									onclick={() => askChangeset('discard_changes', cs.id)}
								>
									{waiting && busy === 'discard_changes' ? 'Abandon…' : 'Abandonner'}
								</Button>
							</div>
						{/if}
					</div>
				{/each}
			</section>

			<section class="flex flex-col gap-2" aria-labelledby={`${safeId}-commands`}>
				<h3 id={`${safeId}-commands`} class="text-xs font-medium text-muted-foreground">Commandes</h3>
				{#if result && result.commands.length > 0}
					<ul class="flex flex-col gap-1 text-xs">
						{#each result.commands as command, i (i)}
							<li class="flex min-w-0 items-center gap-2">
								<span class="min-w-0 flex-1 truncate font-mono">{command.command}</span>
								{#if command.is_error}
									<span class="shrink-0 text-destructive">erreur</span>
								{/if}
								<span class="shrink-0 tabular-nums text-muted-foreground">{formatDuration(command.duration_ms)}</span>
							</li>
						{/each}
					</ul>
				{:else}
					<p class="text-xs text-muted-foreground">{result ? 'Aucune commande.' : 'Aucune commande pour l’instant.'}</p>
				{/if}
			</section>

			<Collapsible.Root class="border-t border-border pt-3">
				<Collapsible.Trigger
					class="group flex items-center gap-1.5 rounded-sm text-xs font-medium text-muted-foreground hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
				>
					<ChevronDown class="size-3.5 -rotate-90 transition-transform group-data-[state=open]:rotate-0" aria-hidden="true" />
					Détails
				</Collapsible.Trigger>
				<Collapsible.Content class="flex flex-col gap-5 pt-3">
					<section class="flex flex-col gap-2">
						<h3 class="text-xs font-medium text-muted-foreground">Mission</h3>
						<MarkdownView source={spawn.task} idPrefix={taskPrefix} />
					</section>

					<section class="flex flex-col gap-2">
						<h3 class="text-xs font-medium text-muted-foreground">Identité</h3>
						<dl class="grid grid-cols-[6.5rem_minmax(0,1fr)] gap-x-3 gap-y-1.5 text-xs">
							<dt class="text-muted-foreground">Profondeur</dt>
							<dd class="tabular-nums">{spawn.depth}</dd>
							<dt class="text-muted-foreground">Profil</dt>
							<dd class="font-mono break-all">
								{spawn.profile} · {spawn.profile_source} · révision {spawn.profile_revision}
							</dd>
							{#if agent.execution > 1}
								<dt class="text-muted-foreground">Exécution</dt>
								<dd class="tabular-nums">{agent.execution} (reprise)</dd>
							{/if}
							<dt class="text-muted-foreground">Démarré</dt>
							<dd class="tabular-nums">{formatTime(agent.spawnedAt)}</dd>
							<dt class="text-muted-foreground">Durée</dt>
							<dd class="tabular-nums">
								{formatDuration(durationMs)}{live ? ' (en cours)' : ''}
							</dd>
						</dl>
					</section>

					<section class="flex flex-col gap-2">
						<h3 class="text-xs font-medium text-muted-foreground">Modèle et raisonnement</h3>
						<dl class="grid grid-cols-[6.5rem_minmax(0,1fr)] gap-x-3 gap-y-1.5 text-xs">
							<dt class="text-muted-foreground">Modèle</dt>
							<dd class="min-w-0">
								<span class="font-mono break-all">{spawn.model.applied}</span>
								{#if modelNote}
									<span class="block text-muted-foreground">{modelNote}</span>
								{/if}
							</dd>
							<dt class="text-muted-foreground">Raisonnement</dt>
							<dd class="min-w-0">
								<span class="font-mono break-all">{spawn.reasoning.applied || 'par défaut'}</span>
								{#if reasoningNote}
									<span class="block text-muted-foreground">{reasoningNote}</span>
								{/if}
							</dd>
						</dl>
					</section>

					<section class="flex flex-col gap-2">
						<h3 class="text-xs font-medium text-muted-foreground">Espace de travail</h3>
						<dl class="grid grid-cols-[6.5rem_minmax(0,1fr)] gap-x-3 gap-y-1.5 text-xs">
							<dt class="text-muted-foreground">Isolation</dt>
							<dd>{isolationLabel(spawn.isolation)}</dd>
							{#if spawn.branch}
								<dt class="text-muted-foreground">Branche</dt>
								<dd class="font-mono break-all">{spawn.branch}</dd>
							{/if}
							<dt class="text-muted-foreground">Dossier</dt>
							<dd class="font-mono break-all">{spawn.workspace}</dd>
						</dl>
					</section>

					{#if agent.tools.length > 0}
						<section class="flex flex-col gap-2">
							<h3 class="flex items-center gap-2 text-xs font-medium text-muted-foreground">
								Appels d’outils
								<span class="font-mono tabular-nums">{agent.tools.length}</span>
							</h3>
							<ul class="flex flex-col gap-0.5">
								{#each agent.tools as tool, i (tool.toolCallId || i)}
									<li>
										<ToolLine
											name={tool.name}
											state={toolLineState(tool, agentInterrupted)}
											durationMs={tool.durationMs}
										/>
									</li>
								{/each}
							</ul>
						</section>
					{/if}

					{#if result}
						<dl class="grid grid-cols-[6.5rem_minmax(0,1fr)] gap-x-3 gap-y-1 text-xs">
							<dt class="text-muted-foreground">Tours</dt>
							<dd class="tabular-nums">{formatCount(result.turns)}</dd>
							<dt class="text-muted-foreground">Appels d’outils</dt>
							<dd class="tabular-nums">{formatCount(result.tool_calls)}</dd>
							<dt class="text-muted-foreground">Durée</dt>
							<dd class="tabular-nums">{formatDuration(result.duration_ms)}</dd>
							<dt class="text-muted-foreground">Jetons</dt>
							<dd class="tabular-nums">
								{plural(result.usage.input_tokens, 'jeton en entrée', 'jetons en entrée')} ·
								{plural(result.usage.output_tokens, 'jeton en sortie', 'jetons en sortie')}
							</dd>
						</dl>
					{/if}
				</Collapsible.Content>
			</Collapsible.Root>
		</div>
	</Dialog.Content>
</Dialog.Root>

<AlertDialog.Root bind:open={confirmOpen}>
	<AlertDialog.Content class="sm:max-w-md">
		<AlertDialog.Header>
			{#if confirmTarget?.action === 'discard_changes'}
				<AlertDialog.Title>Abandonner les modifications de « {spawn.profile} » ?</AlertDialog.Title>
				<AlertDialog.Description>
					Les modifications isolées de cet agent seront supprimées. Cette action est irréversible.
				</AlertDialog.Description>
			{:else}
				<AlertDialog.Title>Appliquer les modifications de « {spawn.profile} » ?</AlertDialog.Title>
				<AlertDialog.Description>
					Ces fichiers seront écrits dans le projet. Vérifiez la liste avant de confirmer.
				</AlertDialog.Description>
			{/if}
		</AlertDialog.Header>

		{#if confirmChangeset && confirmChangeset.files.length > 0}
			<!-- Scrollable region: it must be keyboard-focusable (WAI). -->
			<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
			<div
				role="region"
				aria-label="Fichiers concernés"
				tabindex="0"
				class="max-h-56 overflow-y-auto rounded-md border border-border p-2 outline-none focus-visible:ring-2 focus-visible:ring-ring"
			>
				<p class="mb-1.5 text-xs text-muted-foreground">
					{plural(confirmChangeset.files.length, 'fichier', 'fichiers')}
				</p>
				{@render fileRows(confirmChangeset.files)}
			</div>
		{/if}

		<AlertDialog.Footer>
			<AlertDialog.Cancel>Annuler</AlertDialog.Cancel>
			{#if confirmTarget?.action === 'discard_changes'}
				<AlertDialog.Action variant="destructive" onclick={confirmChangesetAction}>Abandonner</AlertDialog.Action>
			{:else}
				<AlertDialog.Action onclick={confirmChangesetAction}>Appliquer</AlertDialog.Action>
			{/if}
		</AlertDialog.Footer>
	</AlertDialog.Content>
</AlertDialog.Root>

<AlertDialog.Root bind:open={cancelOpen}>
	<AlertDialog.Content class="sm:max-w-md">
		<AlertDialog.Header>
			<AlertDialog.Title>Arrêter ce sous-agent ?</AlertDialog.Title>
			<AlertDialog.Description>« {spawn.profile} » sera arrêté.</AlertDialog.Description>
		</AlertDialog.Header>
		<AlertDialog.Footer>
			<AlertDialog.Cancel>Annuler</AlertDialog.Cancel>
			<AlertDialog.Action variant="destructive" onclick={confirmCancel}>Arrêter l’agent</AlertDialog.Action>
		</AlertDialog.Footer>
	</AlertDialog.Content>
</AlertDialog.Root>
