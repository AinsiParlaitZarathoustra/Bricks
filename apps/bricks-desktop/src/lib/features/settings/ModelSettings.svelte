<script lang="ts">
	import { app } from '$lib/state/app.svelte';
	import { Badge } from '$lib/components/ui/badge';
	import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
	import CircleCheck from '@lucide/svelte/icons/circle-check';
	import SettingsGroup from './SettingsGroup.svelte';
	import { formatCount, formatModalities, formatTokens, providersFilePath } from './format';

	const info = $derived(app.info);
	const configErrors = $derived(info?.config_errors ?? []);
	const providersFile = $derived(info ? providersFilePath(info.bricks_home) : 'providers.toml');
</script>

<div class="flex flex-col gap-6">
	<p class="text-sm text-pretty text-muted-foreground">
		Les fournisseurs et leurs modèles se configurent dans
		<code class="rounded bg-code-bg px-1 py-0.5 font-mono text-xs text-code-foreground">{providersFile}</code>.
		Modifiez ce fichier puis relancez Bricks pour l’appliquer.
	</p>

	{#if configErrors.length > 0}
		<div
			role="alert"
			class="flex flex-col gap-2 rounded-lg border border-destructive/40 bg-destructive/5 p-3 text-sm text-foreground"
		>
			<p class="flex items-center gap-2 font-medium">
				<TriangleAlert class="size-4 text-destructive" aria-hidden="true" />
				{configErrors.length === 1
					? 'Une erreur de configuration'
					: `${formatCount(configErrors.length)} erreurs de configuration`}
			</p>
			<ul class="flex list-disc flex-col gap-1 pl-6 text-pretty">
				{#each configErrors as problem, i (i)}
					<li>{problem}</li>
				{/each}
			</ul>
		</div>
	{/if}

	<SettingsGroup title="Fournisseurs" description="Fournisseurs déclarés. Les clés d’API ne sont jamais affichées.">
		{#if !info}
			<p class="p-3 text-sm text-muted-foreground">Informations indisponibles.</p>
		{:else if info.providers.length === 0}
			<p class="p-3 text-sm text-muted-foreground">Aucun fournisseur déclaré.</p>
		{:else}
			<div class="overflow-x-auto">
				<table class="w-full min-w-[32rem] text-left text-sm">
					<caption class="sr-only">Fournisseurs configurés</caption>
					<thead class="text-xs text-muted-foreground">
						<tr class="border-b border-border">
							<th scope="col" class="px-3 py-2 font-medium">Fournisseur</th>
							<th scope="col" class="px-3 py-2 font-medium">Identifiants</th>
							<th scope="col" class="px-3 py-2 text-right font-medium">Modèles</th>
						</tr>
					</thead>
					<tbody>
						{#each info.providers as provider (provider.id)}
							<tr class="border-b border-border last:border-b-0">
								<th scope="row" class="px-3 py-2 font-normal">
									<span class="block text-foreground">{provider.name}</span>
									<span class="block font-mono text-xs text-muted-foreground">{provider.id}</span>
								</th>
								<td class="px-3 py-2">
									{#if provider.has_credentials}
										<span class="inline-flex items-center gap-1.5 text-success">
											<CircleCheck class="size-4" aria-hidden="true" />
											Présents
										</span>
									{:else}
										<span class="inline-flex items-center gap-1.5 text-warning">
											<TriangleAlert class="size-4" aria-hidden="true" />
											Absents
										</span>
									{/if}
								</td>
								<td class="px-3 py-2 text-right font-mono tabular-nums">{formatCount(provider.model_count)}</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}
	</SettingsGroup>

	<SettingsGroup title="Modèles disponibles" description="Catalogue fourni par les fournisseurs configurés.">
		{#if app.models.length === 0}
			<p class="p-3 text-sm text-muted-foreground">Aucun modèle disponible.</p>
		{:else}
			<div class="overflow-x-auto">
				<table class="w-full min-w-[40rem] text-left text-sm">
					<caption class="sr-only">Modèles disponibles</caption>
					<thead class="text-xs text-muted-foreground">
						<tr class="border-b border-border">
							<th scope="col" class="px-3 py-2 font-medium">Modèle</th>
							<th scope="col" class="px-3 py-2 font-medium">Fenêtre de contexte</th>
							<th scope="col" class="px-3 py-2 font-medium">Entrée maximale</th>
							<th scope="col" class="px-3 py-2 font-medium">Entrées acceptées</th>
							<th scope="col" class="px-3 py-2 font-medium">Profils</th>
						</tr>
					</thead>
					<tbody>
						{#each app.models as model (model.selection)}
							<tr class="border-b border-border align-top last:border-b-0">
								<th scope="row" class="px-3 py-2 font-normal">
									<span class="block text-foreground">{model.name}</span>
									<span class="block font-mono text-xs break-all text-muted-foreground">{model.selection}</span>
								</th>
								<td class="px-3 py-2 font-mono text-xs whitespace-nowrap tabular-nums">
									{formatTokens(model.context_window_tokens)}
								</td>
								<td class="px-3 py-2 font-mono text-xs whitespace-nowrap tabular-nums">
									{formatTokens(model.max_input_tokens)}
								</td>
								<td class="px-3 py-2 text-xs">{formatModalities(model.input)}</td>
								<td class="px-3 py-2 text-xs">
									{#if model.profiles.length === 0}
										<span class="text-muted-foreground">Aucun</span>
									{:else}
										<ul class="flex flex-col gap-0.5">
											{#each model.profiles as profile (profile.id)}
												<li>
													{profile.label}
													{#if profile.id === model.default_profile}
														<span class="text-muted-foreground">(par défaut)</span>
													{/if}
												</li>
											{/each}
										</ul>
									{/if}
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}
	</SettingsGroup>

	<SettingsGroup title="Choix par défaut" description="Modèle et effort de raisonnement appliqués aux nouvelles conversations.">
		<div class="flex items-center justify-between gap-4 p-3">
			<span class="text-sm text-muted-foreground">Modèle par défaut</span>
			<Badge variant="outline">À venir</Badge>
		</div>
		<div class="flex items-center justify-between gap-4 p-3">
			<span class="text-sm text-muted-foreground">Effort de raisonnement par défaut</span>
			<Badge variant="outline">À venir</Badge>
		</div>
	</SettingsGroup>
</div>
