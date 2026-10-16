<script lang="ts">
	import { onMount, untrack } from 'svelte';
	import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';
	import SquareIcon from '@lucide/svelte/icons/square';
	import * as AlertDialog from '$lib/components/ui/alert-dialog';
	import { Button } from '$lib/components/ui/button';
	import { formatDuration, formatTime, plural } from '$lib/features/agents/agentView';
	import StatusDot from '$lib/layout/StatusDot.svelte';
	import { app } from '$lib/state/app.svelte';
	import type { InspectorState } from '$lib/state/ui.svelte';
	import { formatBytes, jobDurationMs, jobStatus, sortJobs } from './labels';
	import { EngineRequest, controlSlot, type ControlAnswer } from './request.svelte';

	// Opened without options: the view always shows the engine's jobs.
	let { args: _args }: { args: InspectorState['args'] } = $props();

	const conv = $derived(app.current);
	/** Background jobs of this conversation, as the engine reports them. */
	const jobs = $derived(conv ? sortJobs([...conv.jobs.values()]) : []);
	const anyRunning = $derived(jobs.some((job) => job.state === 'running'));

	// One clock for the list, running only while a job runs.
	let now = $state(Date.now());

	$effect(() => {
		if (!anyRunning) return;
		untrack(() => (now = Date.now()));
		const id = setInterval(() => (now = Date.now()), 1000);
		return () => clearInterval(id);
	});

	const refresh = new EngineRequest();
	const refreshAnswer = $derived(refresh.answered<ControlAnswer>());
	const stop = new EngineRequest();
	const stopAnswer = $derived(stop.answered<ControlAnswer>());

	let confirmOpen = $state(false);
	let confirmJob = $state<string | null>(null);
	const confirmCommand = $derived(
		confirmJob ? (jobs.find((job) => job.jobId === confirmJob)?.command ?? '') : ''
	);

	function refreshJobs() {
		void refresh.send({ type: 'agent_control', action: 'jobs' }, controlSlot, 'jobs');
	}

	function askStop(jobId: string) {
		confirmJob = jobId;
		confirmOpen = true;
	}

	function stopConfirmed() {
		const jobId = confirmJob;
		confirmJob = null;
		if (jobId) {
			void stop.send(
				{ type: 'agent_control', action: 'stop_job', job_id: jobId },
				controlSlot,
				'stop_job'
			);
		}
	}

	onMount(() => refreshJobs());
</script>

<div class="flex flex-col gap-4 text-sm">
	<p class="text-xs text-pretty text-muted-foreground">
		Tâches d’arrière-plan lancées par les agents de cette conversation. Ce ne sont pas vos
		terminaux.
	</p>

	{#if !conv}
		<p class="text-muted-foreground">Aucune conversation ouverte.</p>
	{:else}
		<div class="flex flex-wrap items-center justify-between gap-2">
			<span role="status" aria-live="polite" class="text-xs text-muted-foreground">
				{#if refresh.pending}
					Actualisation…
				{:else}
					{plural(jobs.length, 'tâche', 'tâches')}
				{/if}
			</span>
			<Button variant="outline" size="sm" disabled={refresh.pending} onclick={refreshJobs}>
				<RefreshCwIcon aria-hidden="true" />
				Actualiser
			</Button>
		</div>

		{#if refresh.refusal}
			<p role="alert" class="rounded-md bg-destructive/10 px-3 py-2 text-xs text-pretty text-destructive">
				Commande refusée : {refresh.refusal}
			</p>
		{/if}

		{#if stop.refusal}
			<p role="alert" class="rounded-md bg-destructive/10 px-3 py-2 text-xs text-pretty text-destructive">
				Arrêt refusé : {stop.refusal}
			</p>
		{:else if stop.pending}
			<p role="status" class="text-xs text-muted-foreground">Arrêt demandé, réponse du moteur en attente…</p>
		{:else if stopAnswer}
			{#if stopAnswer.ok}
				<p role="status" class="text-xs text-pretty text-muted-foreground">
					Réponse du moteur : {stopAnswer.text}
				</p>
			{:else}
				<p role="alert" class="rounded-md bg-destructive/10 px-3 py-2 text-xs text-pretty text-destructive">
					Arrêt impossible : {stopAnswer.text}
				</p>
			{/if}
		{/if}

		{#if refreshAnswer}
			{#if refreshAnswer.ok}
				<section aria-label="Réponse du moteur" class="flex flex-col gap-1">
					<h3 class="text-xs font-medium text-muted-foreground">Réponse du moteur</h3>
					<pre class="max-h-48 overflow-auto rounded-md bg-code-bg px-2.5 py-2 font-mono text-xs whitespace-pre-wrap text-code-foreground">{refreshAnswer.text}</pre>
				</section>
			{:else}
				<p role="alert" class="rounded-md bg-destructive/10 px-3 py-2 text-xs text-pretty text-destructive">
					Actualisation impossible : {refreshAnswer.text}
				</p>
			{/if}
		{/if}

		{#if jobs.length === 0}
			{#if !refresh.pending}
				<p class="text-muted-foreground">Aucune tâche d’arrière-plan connue pour cette conversation.</p>
			{/if}
		{:else}
			<!-- A named, focusable scroll container: keyboard users must be able to scroll
				it (WCAG 2.1.1). The rule only checks the role, so it is suppressed here. -->
			<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
			<div
				role="region"
				aria-label="Tâches d’arrière-plan"
				tabindex="0"
				class="flex max-h-[52vh] flex-col gap-3 overflow-y-auto rounded-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
			>
				<ul class="flex flex-col gap-2">
					{#each jobs as job (job.jobId)}
						{@const status = jobStatus(job.state)}
						<li class="flex min-w-0 flex-col gap-2 rounded-md border border-border p-2.5">
							<div class="flex min-w-0 items-center gap-2">
								<span class="min-w-0 flex-1 truncate font-mono text-xs" title={job.command}>{job.command}</span>
								<StatusDot status={status.dot} label={status.label} showLabel class="shrink-0" />
							</div>
							<dl class="grid grid-cols-[6.5rem_minmax(0,1fr)] gap-x-3 gap-y-1 text-xs">
								<dt class="text-muted-foreground">Démarrée</dt>
								<dd class="tabular-nums">{formatTime(job.startedAt)}</dd>
								<dt class="text-muted-foreground">Durée</dt>
								<dd class="tabular-nums">
									{formatDuration(jobDurationMs(job, now))}{job.state === 'running' ? ' (en cours)' : ''}
								</dd>
								<dt class="text-muted-foreground">Dossier</dt>
								<dd class="font-mono break-all">{job.cwd}</dd>
								<dt class="text-muted-foreground">PID</dt>
								<dd class="tabular-nums">{job.pid}</dd>
								<dt class="text-muted-foreground">Agent</dt>
								<dd class="font-mono break-all">{job.agentId}</dd>
								<dt class="text-muted-foreground">Sortie</dt>
								<dd class="tabular-nums">
									{formatBytes(job.stdoutBytes)} (stdout) · {formatBytes(job.stderrBytes)} (stderr)
								</dd>
								{#if job.code !== undefined}
									<dt class="text-muted-foreground">Code</dt>
									<dd class="tabular-nums">{job.code}</dd>
								{/if}
								{#if job.signal !== undefined}
									<dt class="text-muted-foreground">Signal</dt>
									<dd class="tabular-nums">{job.signal}</dd>
								{/if}
								{#if job.logs.length > 0}
									<dt class="text-muted-foreground">Journaux</dt>
									<dd class="font-mono break-all">{job.logs.join(', ')}</dd>
								{/if}
							</dl>
							{#if job.state === 'running'}
								<div>
									<Button
										variant="outline"
										size="sm"
										disabled={stop.pending}
										onclick={() => askStop(job.jobId)}
									>
										<SquareIcon aria-hidden="true" />
										Arrêter
									</Button>
								</div>
							{/if}
						</li>
					{/each}
				</ul>
			</div>
		{/if}
	{/if}
</div>

<AlertDialog.Root bind:open={confirmOpen}>
	<AlertDialog.Content>
		<AlertDialog.Header>
			<AlertDialog.Title>Arrêter cette tâche ?</AlertDialog.Title>
			<AlertDialog.Description class="font-mono break-all">
				{confirmCommand}
			</AlertDialog.Description>
		</AlertDialog.Header>
		<AlertDialog.Footer>
			<AlertDialog.Cancel>Annuler</AlertDialog.Cancel>
			<AlertDialog.Action variant="destructive" onclick={stopConfirmed}>Arrêter</AlertDialog.Action>
		</AlertDialog.Footer>
	</AlertDialog.Content>
</AlertDialog.Root>
