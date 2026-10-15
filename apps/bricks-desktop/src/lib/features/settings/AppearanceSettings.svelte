<script lang="ts">
	import { app } from '$lib/state/app.svelte';
	import { setTheme, theme, type ThemeMode } from '$lib/theme/theme.svelte';
	import SettingsGroup from './SettingsGroup.svelte';
	import Sun from '@lucide/svelte/icons/sun';
	import Moon from '@lucide/svelte/icons/moon';
	import Monitor from '@lucide/svelte/icons/monitor';
	import type { Component } from 'svelte';

	const OPTIONS: {
		value: ThemeMode;
		label: string;
		description: string;
		icon: Component<{ class?: string; 'aria-hidden'?: boolean | 'true' | 'false' }>;
	}[] = [
		{ value: 'light', label: 'Clair', description: 'Fond clair en permanence.', icon: Sun },
		{ value: 'dark', label: 'Sombre', description: 'Fond sombre en permanence.', icon: Moon },
		{
			value: 'system',
			label: 'Système',
			description: 'Suit le réglage de couleur du système.',
			icon: Monitor
		}
	];

	const current = $derived(app.prefs.theme);

	/** Applies the mode at once and saves it with the other preferences. */
	function choose(mode: ThemeMode) {
		setTheme(mode);
		app.setPrefs({ theme: mode });
	}
</script>

<div class="flex flex-col gap-6">
	<SettingsGroup title="Thème" description="Choisissez l’apparence de Bricks. Le choix est enregistré.">
		<fieldset class="flex flex-col gap-0 p-0">
			<legend class="sr-only">Thème</legend>
			<div class="grid gap-3 p-3 sm:grid-cols-3">
				{#each OPTIONS as option (option.value)}
					{@const Icon = option.icon}
					<label
						class="relative flex cursor-pointer flex-col gap-2 rounded-lg border border-border bg-background p-3 text-sm transition-colors hover:bg-muted has-[:checked]:border-primary has-[:checked]:ring-1 has-[:checked]:ring-primary has-[:focus-visible]:ring-2 has-[:focus-visible]:ring-ring"
					>
						<input
							type="radio"
							name="theme-mode"
							value={option.value}
							class="sr-only"
							checked={current === option.value}
							onchange={() => choose(option.value)}
						/>
						<span class="flex items-center gap-2 font-medium text-foreground">
							<Icon class="size-4" aria-hidden="true" />
							{option.label}
						</span>
						<span class="text-xs text-pretty text-muted-foreground">{option.description}</span>
					</label>
				{/each}
			</div>
		</fieldset>
		<p class="px-3 pb-3 text-xs text-muted-foreground" aria-live="polite">
			Affichage actuel : {theme.resolved === 'dark' ? 'sombre' : 'clair'}.
		</p>
	</SettingsGroup>
</div>
