<script lang="ts">
	// Context occupation (`/context`, `/ctx`): the same compact line as the context
	// card, and explicit compaction and clearing. Owner: Haiku 1 (Sprint 14.2),
	// after Haiku 4 (Sprint 14.1).
	import type { InspectorState } from '$lib/state/ui.svelte';
	import { app } from '$lib/state/app.svelte';
	import { ui } from '$lib/state/ui.svelte';
	import { Button } from '$lib/components/ui/button';
	import { contextLine } from '$lib/features/context/contextView';

	interface Props {
		/** Inspector arguments; the view has a single presentation, so none changes it. */
		args: InspectorState['args'];
	}

	let { args }: Props = $props();

	const conv = $derived(app.current);
	const status = $derived(conv?.context ?? null);
	const stale = $derived(conv?.contextStale ?? false);
	const line = $derived(contextLine(status, stale));
	const busy = $derived(conv ? conv.running || conv.busy !== null : false);

	// Compaction: the answer is the `compaction` event newer than the request,
	// or a `command_rejected` for `compact` newer than the last rejection seen.
	let compactSent = $state<{ compactAt: number | null; refusedAt: number | null } | null>(null);
	let actionError = $state<string | null>(null);

	const compactDone = $derived.by(() => {
		const c = conv?.lastCompaction;
		if (!compactSent || !c) return null;
		return compactSent.compactAt === null || c.at > compactSent.compactAt ? c : null;
	});
	const compactRefused = $derived.by(() => {
		const r = conv?.lastRejection;
		if (!compactSent || !r || r.command !== 'compact') return null;
		return compactSent.refusedAt === null || r.at > compactSent.refusedAt ? r.reason : null;
	});
	const compactPending = $derived(compactSent !== null && compactDone === null && compactRefused === null);

	async function compact() {
		const current = app.current;
		if (!current) return;
		actionError = null;
		compactSent = {
			compactAt: current.lastCompaction?.at ?? null,
			refusedAt: current.lastRejection?.at ?? null
		};
		const refused = await app.command(current.sessionId, { type: 'compact' });
		if (refused !== null) {
			compactSent = null;
			actionError = refused;
		}
	}
</script>

<div class="flex min-w-0 flex-col gap-4">
	<section class="flex flex-col gap-1" aria-label="Occupation du contexte">
		<p class="text-base font-medium tabular-nums">{line}</p>
	</section>

	<div class="flex flex-wrap gap-2">
		<Button variant="outline" size="sm" onclick={compact} disabled={busy || compactPending}>
			{compactPending ? 'Compaction en cours…' : 'Compacter maintenant'}
		</Button>
		<Button variant="outline" size="sm" onclick={() => ui.open('clear')}>Vider le contexte…</Button>
	</div>

	<div class="flex flex-col gap-2 text-sm" role="status" aria-live="polite">
		{#if busy}
			<p class="text-xs text-muted-foreground">
				Une exécution ou une opération est en cours : compaction et vidage attendent sa fin.
			</p>
		{/if}
		{#if compactPending}
			<p class="text-muted-foreground">
				Compaction demandée. Le résultat apparaîtra ici dès que le moteur répond.
			</p>
		{:else if compactRefused !== null}
			<p role="alert" class="text-destructive">Compaction refusée : {compactRefused}</p>
		{:else if compactDone}
			<p>
				{compactDone.compacted ? 'Contexte compacté.' : 'Contexte non compacté.'}
				<span class="text-muted-foreground">Résultat du moteur : {compactDone.outcome}</span>
			</p>
		{/if}
		{#if actionError}
			<p role="alert" class="text-destructive">{actionError}</p>
		{/if}
	</div>
</div>
