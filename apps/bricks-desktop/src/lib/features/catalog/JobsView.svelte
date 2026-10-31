<script lang="ts">
	import { onMount, untrack } from 'svelte';
	import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
	import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';
	import SquareIcon from '@lucide/svelte/icons/square';
	import * as AlertDialog from '$lib/components/ui/alert-dialog';
	import { Button } from '$lib/components/ui/button';
	import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '$lib/components/ui/collapsible';
	import { formatDuration, formatTime } from '$lib/features/agents/agentView';
	import StatusDot from '$lib/layout/StatusDot.svelte';
	import { app } from '$lib/state/app.svelte';
	import type { InspectorState } from '$lib/state/ui.svelte';
	import { formatBytes, jobDurationMs, jobStatus, jobsSummary, sortJobs } from './labels';
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
	const summary = $derived(refresh.pending && jobs.length === 0 ? 'Actualisation…' : jobsSummary(jobs));
	/** Folded: the summary says the state; the list and each job's details open on demand. */
	let open = $state(false);

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
		confirmOpen = false;
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

<div class="flex flex-col gap-3 text-ui">
	{#if !conv}
		<p class="text-muted-foreground">Aucune conversation ouverte</p>
	{:else}
		<section aria-label="Tâches de fond" class="rounded-xl border border-border bg-card text-card-foreground">
			<Collapsible bind:open>
				<div class="flex min-w-0 items-center gap-1 pr-1.5">
					<CollapsibleTrigger
						disabled={jobs.length === 0}
						class="group flex min-w-0 flex-1 items-center gap-2 rounded-xl px-3 py-2 text-start enabled:hover:bg-muted/50 focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
					>
						<ChevronRightIcon
							class="size-3.5 shrink-0 text-muted-foreground transition-transform group-data-[state=open]:rotate-90 group-disabled:opacity-0"
							aria-hidden="true"
						/>
						<span class="min-w-0 truncate tabular-nums">{summary}</span>
					</CollapsibleTrigger>
					<Button
						variant="ghost"
						size="icon-xs"
						aria-label="Actualiser"
						title="Actualiser"
						disabled={refresh.pending}
						onclick={refreshJobs}
					>
						<RefreshCwIcon class={refresh.pending ? 'animate-spin' : ''} aria-hidden="true" />
					</Button>
				</div>
				<CollapsibleContent>
					<ul class="max-h-[52vh] overflow-y-auto border-t border-border">
						{#each jobs as job (job.jobId)}
							{@const status = jobStatus(job.state)}
							<li class="border-b border-border last:border-b-0">
								<Collapsible>
									<div class="flex min-w-0 items-center gap-1 pr-1.5">
										<CollapsibleTrigger
											class="group flex min-w-0 flex-1 items-center gap-2 px-3 py-1.5 text-start hover:bg-muted/50 focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none"
										>
											<ChevronRightIcon
												class="size-3.5 shrink-0 text-muted-foreground transition-transform group-data-[state=open]:rotate-90"
												aria-hidden="true"
											/>
											<StatusDot status={status.dot} label={status.label} class="shrink-0" />
											<span class="min-w-0 flex-1 truncate font-mono text-xs" title={job.command}>{job.command}</span>
											<span class="shrink-0 text-xs tabular-nums text-muted-foreground">
												{formatDuration(jobDurationMs(job, now))}
											</span>
										</CollapsibleTrigger>
										{#if job.state === 'running'}
											<Button
												variant="ghost"
												size="icon-xs"
												aria-label={`Arrêter ${job.command}`}
												title="Arrêter"
												disabled={stop.pending}
												onclick={() => askStop(job.jobId)}
											>
												<SquareIcon aria-hidden="true" />
											</Button>
										{/if}
									</div>
									<CollapsibleContent>
										<dl class="grid grid-cols-[5.5rem_minmax(0,1fr)] gap-x-3 gap-y-1 px-3 pt-0.5 pb-2 pl-9 text-xs">
											<dt class="text-muted-foreground">État</dt>
											<dd>{status.label}</dd>
											<dt class="text-muted-foreground">Démarrée</dt>
											<dd class="tabular-nums">{formatTime(job.startedAt)}</dd>
											<dt class="text-muted-foreground">Dossier</dt>
											<dd class="font-mono break-all">{job.cwd}</dd>
											<dt class="text-muted-foreground">PID</dt>
											<dd class="tabular-nums">{job.pid}</dd>
											<dt class="text-muted-foreground">Agent</dt>
											<dd class="font-mono break-all">{job.agentId}</dd>
											<dt class="text-muted-foreground">Sortie</dt>
											<dd class="tabular-nums">
												{formatBytes(job.stdoutBytes)} stdout · {formatBytes(job.stderrBytes)} stderr
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
									</CollapsibleContent>
								</Collapsible>
							</li>
						{/each}
					</ul>
				</CollapsibleContent>
			</Collapsible>
		</section>

		{#if refresh.refusal}
			<p role="alert" class="text-xs text-pretty text-destructive">Actualisation refusée : {refresh.refusal}</p>
		{:else if refreshAnswer && !refreshAnswer.ok}
			<p role="alert" class="text-xs text-pretty text-destructive">Actualisation impossible : {refreshAnswer.text}</p>
		{/if}
		{#if stop.refusal}
			<p role="alert" class="text-xs text-pretty text-destructive">Arrêt refusé : {stop.refusal}</p>
		{:else if stop.pending}
			<p role="status" class="text-xs text-muted-foreground">Arrêt demandé</p>
		{:else if stopAnswer && !stopAnswer.ok}
			<p role="alert" class="text-xs text-pretty text-destructive">Arrêt impossible : {stopAnswer.text}</p>
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
