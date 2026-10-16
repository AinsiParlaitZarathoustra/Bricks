<script lang="ts">
	// Effective configuration of the session (`/config`): model, permissions,
	// memory, sub-agents, configuration files read, other settings. Shows no key
	// and no authentication header: the engine never sends them. Owner: Haiku 6 (Sprint 14.1).
	import { untrack } from 'svelte';
	import { Badge } from '$lib/components/ui/badge';
	import { errorMessage, sessionConfig, type SessionConfigInfo } from '$lib/ipc/client';
	import EmptyState from '$lib/layout/EmptyState.svelte';
	import SectionCard from '$lib/layout/SectionCard.svelte';
	import { app } from '$lib/state/app.svelte';
	import type { InspectorState } from '$lib/state/ui.svelte';
	import { ruleLabel, rulesView } from './format';

	// Opened without options: the view shows the conversation on screen.
	let { args: _args }: { args: InspectorState['args'] } = $props();

	const conv = $derived(app.current);
	const sessionId = $derived(conv?.sessionId ?? null);

	let config = $state<SessionConfigInfo | null>(null);
	let loadError = $state<string | null>(null);

	/** Only the newest read may write the view (a slower earlier one is dropped). */
	let loadToken = 0;

	async function load(id: string) {
		const token = ++loadToken;
		try {
			const next = await sessionConfig(id);
			if (token !== loadToken) return;
			config = next;
			loadError = null;
		} catch (e) {
			if (token !== loadToken) return;
			loadError = errorMessage(e);
		}
	}

	// Read on open, and again when a run ends or an approval is answered: an
	// "allow for this session" grant changes the permissions shown.
	$effect(() => {
		const id = sessionId;
		void conv?.running;
		void conv?.approvals.size;
		if (!id) {
			loadToken++;
			config = null;
			loadError = null;
			return;
		}
		untrack(() => void load(id));
	});

	const rules = $derived(config ? rulesView(config.approval_rules) : null);
	const WARNINGS_LABEL = 'Avertissements de configuration';
</script>

{#snippet field(label: string, value: string)}
	<dt class="text-muted-foreground">{label}</dt>
	<dd class="min-w-0 break-words">{value}</dd>
{/snippet}

{#if !conv || !sessionId}
	<EmptyState
		title="Aucune conversation ouverte"
		description="Ouvrez une conversation pour voir sa configuration."
	/>
{:else if loadError}
	<p role="alert" class="text-sm text-destructive">Lecture de la configuration impossible : {loadError}</p>
{:else if !config}
	<p class="text-sm text-muted-foreground" aria-live="polite">Chargement de la configuration…</p>
{:else}
	<div class="flex flex-col gap-3">
		<SectionCard title="Modèle">
			<dl class="grid grid-cols-[auto_minmax(0,1fr)] items-baseline gap-x-6 gap-y-2 px-3 pb-3 text-sm">
				{@render field('Fournisseur', config.provider || 'inconnu')}
				{@render field('Modèle', config.model || 'inconnu')}
				{@render field('Effort demandé', config.reasoning ?? 'par défaut du modèle')}
				{@render field('Effort appliqué', config.effective_reasoning ?? 'aucun')}
				{@render field('Dossier de travail', config.working_dir)}
			</dl>
		</SectionCard>

		<SectionCard title="Permissions">
			<div class="flex flex-col gap-4 px-3 pb-3 text-sm">
				<dl class="grid grid-cols-[auto_minmax(0,1fr)] items-baseline gap-x-6 gap-y-2">
					{@render field(
						'Approbations',
						config.interactive
							? 'demandées à une personne'
							: 'aucune personne pour répondre : les actions à approuver sont refusées'
					)}
				</dl>

				{#if rules}
					<div class="flex flex-col gap-2">
						<h4 class="text-xs font-medium text-muted-foreground">Par niveau d’outil</h4>
						<dl class="grid grid-cols-[auto_minmax(0,1fr)] items-baseline gap-x-6 gap-y-2">
							{#each rules.levels as level (level.key)}
								{@render field(level.label, ruleLabel(level.action))}
							{/each}
							{@render field('Autres outils', ruleLabel(rules.defaultAction))}
						</dl>
					</div>

					<div class="flex flex-col gap-2">
						<h4 class="text-xs font-medium text-muted-foreground">Par outil</h4>
						{#if rules.tools.length > 0}
							<ul class="flex flex-col gap-1">
								{#each rules.tools as override, i (i)}
									<li class="flex items-center justify-between gap-3">
										<span class="min-w-0 font-mono text-xs break-all">{override.name}</span>
										<Badge variant="outline">{ruleLabel(override.action)}</Badge>
									</li>
								{/each}
							</ul>
						{:else}
							<p class="text-muted-foreground">Aucune règle par outil.</p>
						{/if}
					</div>
				{:else}
					<p role="alert" class="text-destructive">Règles d’approbation illisibles.</p>
				{/if}

				<div class="flex flex-col gap-2">
					<h4 class="text-xs font-medium text-muted-foreground">Autorisés pour cette session</h4>
					{#if config.allowed_for_session.length > 0}
						<ul class="flex flex-col gap-1">
							{#each config.allowed_for_session as name, i (i)}
								<li class="font-mono text-xs break-all">{name}</li>
							{/each}
						</ul>
					{:else}
						<p class="text-muted-foreground">Aucun outil.</p>
					{/if}
				</div>
			</div>
		</SectionCard>

		<SectionCard title="Mémoire">
			<dl class="grid grid-cols-[auto_minmax(0,1fr)] items-baseline gap-x-6 gap-y-2 px-3 pb-3 text-sm">
				{@render field(
					'Espace',
					config.memory_space ?? 'aucun : mémoire à long terme désactivée (section [memory] de bricks.toml)'
				)}
			</dl>
		</SectionCard>

		<SectionCard title="Agents">
			<dl class="grid grid-cols-[auto_minmax(0,1fr)] items-baseline gap-x-6 gap-y-2 px-3 pb-3 text-sm">
				{@render field(
					'Sous-agents',
					config.agents_enabled ? 'activés (section [agents] de bricks.toml)' : 'désactivés'
				)}
			</dl>
		</SectionCard>

		<SectionCard title="Sources" count={config.sources.length}>
			<div class="flex flex-col gap-2 px-3 pb-3 text-sm">
				{#if config.sources.length > 0}
					<ul class="flex flex-col gap-1">
						{#each config.sources as source, i (i)}
							<li class="font-mono text-xs break-all">{source}</li>
						{/each}
					</ul>
				{:else}
					<p class="text-muted-foreground">
						Aucun fichier de configuration lu : valeurs par défaut.
					</p>
				{/if}
				<p class="text-xs text-muted-foreground">
					Les clés d’accès et les en-têtes d’authentification ne sont jamais affichés.
				</p>
			</div>
		</SectionCard>

		<SectionCard title="Autres réglages" count={config.settings.length}>
			<dl class="grid grid-cols-[auto_minmax(0,1fr)] items-baseline gap-x-6 gap-y-2 px-3 pb-3 text-sm">
				{#each config.settings as entry, i (i)}
					<dt class="text-muted-foreground">{entry.label}</dt>
					<dd class="min-w-0 break-words">
						{entry.value}
						{#if entry.label === WARNINGS_LABEL && entry.value !== '0'}
							<span class="block text-xs text-muted-foreground">
								Les messages détaillés ne sont pas affichés : ils peuvent citer une valeur saisie
								dans bricks.toml.
							</span>
						{/if}
					</dd>
				{/each}
			</dl>
		</SectionCard>
	</div>
{/if}
