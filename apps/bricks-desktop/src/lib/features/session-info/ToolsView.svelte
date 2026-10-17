<script lang="ts">
	// Tools of the session (`/tools`): name, description, permission level, the
	// decision the approval rules give it, and whether it was allowed for the
	// session. Owner: Haiku 6 (Sprint 14.1).
	import { untrack } from 'svelte';
	import { Badge } from '$lib/components/ui/badge';
	import { Input } from '$lib/components/ui/input';
	import { errorMessage, sessionTools, type SessionToolInfo } from '$lib/ipc/client';
	import EmptyState from '$lib/layout/EmptyState.svelte';
	import { app } from '$lib/state/app.svelte';
	import type { InspectorState } from '$lib/state/ui.svelte';
	import {
		decisionLabel,
		decisionVariant,
		levelLabel,
		matchesTool
	} from './format';

	// Opened without options: the view shows the conversation on screen.
	let { args: _args }: { args: InspectorState['args'] } = $props();

	const conv = $derived(app.current);
	const sessionId = $derived(conv?.sessionId ?? null);

	let tools = $state<SessionToolInfo[] | null>(null);
	let loadError = $state<string | null>(null);
	let filter = $state('');

	/** Only the newest read may write the view (a slower earlier one is dropped). */
	let loadToken = 0;

	async function load(id: string) {
		const token = ++loadToken;
		try {
			const next = await sessionTools(id);
			if (token !== loadToken) return;
			tools = next;
			loadError = null;
		} catch (e) {
			if (token !== loadToken) return;
			loadError = errorMessage(e);
		}
	}

	// Read on open, and again when a run ends or an approval is answered: an
	// "allow for this session" grant changes what the table says.
	$effect(() => {
		const id = sessionId;
		void conv?.running;
		void conv?.approvals.size;
		if (!id) {
			loadToken++;
			tools = null;
			loadError = null;
			return;
		}
		untrack(() => void load(id));
	});

	const visible = $derived(tools ? tools.filter((t) => matchesTool(t, filter)) : []);
</script>

{#if !conv || !sessionId}
	<EmptyState
		title="Aucune conversation ouverte"
	/>
{:else if loadError}
	<p role="alert" class="text-sm text-destructive">Lecture des outils impossible : {loadError}</p>
{:else if tools === null}
	<p class="text-sm text-muted-foreground" aria-live="polite">Chargement des outils…</p>
{:else if tools.length === 0}
	<EmptyState title="Aucun outil disponible" />
{:else}
	<div class="flex flex-col gap-3">
		<Input
			type="search"
			bind:value={filter}
			placeholder="Filtrer par nom ou description"
			aria-label="Filtrer les outils"
			autocomplete="off"
		/>

		{#if visible.length === 0}
			<p class="text-sm text-muted-foreground">Aucun outil ne correspond à « {filter.trim()} ».</p>
		{:else}
			<p class="text-xs text-muted-foreground" aria-live="polite">
				{visible.length === tools.length
					? `${tools.length} outils`
					: `${visible.length} sur ${tools.length} outils`}
			</p>
			<div class="overflow-x-auto rounded-lg border border-border">
				<table class="w-full min-w-[32rem] text-left text-sm">
					<caption class="sr-only">Outils de la session, avec leur niveau et leur décision</caption>
					<thead class="bg-muted text-xs text-muted-foreground">
						<tr>
							<th scope="col" class="px-3 py-2 font-medium">Outil</th>
							<th scope="col" class="px-3 py-2 font-medium">Niveau</th>
							<th scope="col" class="px-3 py-2 font-medium">Décision</th>
							<th scope="col" class="px-3 py-2 font-medium">Pour la session</th>
						</tr>
					</thead>
					<tbody class="divide-y divide-border">
						{#each visible as tool, i (i)}
							<tr class="align-top">
								<th scope="row" class="px-3 py-2 font-mono text-xs font-medium break-all">
									{tool.name}
									{#if tool.description}
										<p class="mt-0.5 font-sans text-xs font-normal text-muted-foreground">
											{tool.description}
										</p>
									{/if}
								</th>
								<td class="px-3 py-2 whitespace-nowrap">{levelLabel(tool.level)}</td>
								<td class="px-3 py-2">
									<Badge variant={decisionVariant(tool.decision)}>{decisionLabel(tool.decision)}</Badge>
								</td>
								<td class="px-3 py-2 whitespace-nowrap">
									{#if tool.allowed_for_session}
										<Badge variant="secondary">Autorisé</Badge>
									{:else}
										<span class="text-muted-foreground">non</span>
									{/if}
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}
	</div>
{/if}
