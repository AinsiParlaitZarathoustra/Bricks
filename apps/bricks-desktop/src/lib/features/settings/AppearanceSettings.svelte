<script lang="ts">
	// « Apparence » of the settings: the light, dark or system mode, three colours per mode
	// (accent, central background, panels), saved per resolved mode, and the app's icon.
	import { t } from '$lib/i18n/i18n.svelte';
	import type { AppIcon } from '$lib/ipc/client';
	import { app } from '$lib/state/app.svelte';
	import { setTheme, theme, type ThemeMode } from '$lib/theme/theme.svelte';
	import {
		COLOR_ROLES,
		COLOR_ROLE_LABELS,
		PRESETS,
		colorChecks,
		isCustomRole,
		roleColor,
		roleForeground,
		type ColorRole,
		type Hex,
		type ResolvedMode
	} from '$lib/theme/palette';
	import SettingsGroup from './SettingsGroup.svelte';
	import ColorChoice from './ColorChoice.svelte';
	import Sun from '@lucide/svelte/icons/sun';
	import Moon from '@lucide/svelte/icons/moon';
	import Monitor from '@lucide/svelte/icons/monitor';
	import SunMoon from '@lucide/svelte/icons/sun-moon';
	import type { Component } from 'svelte';

	const OPTIONS: {
		value: ThemeMode;
		label: string;
		icon: Component<{ class?: string; 'aria-hidden'?: boolean | 'true' | 'false' }>;
	}[] = [
		{ value: 'light', get label() { return t('settings.light') }, icon: Sun },
		{ value: 'dark', get label() { return t('settings.dark') }, icon: Moon },
		{ value: 'system', get label() { return t('settings.system') }, icon: Monitor }
	];

	/** The icon: always light, always dark, or the one of the mode on screen. */
	const ICON_OPTIONS: {
		value: AppIcon;
		label: string;
		icon: Component<{ class?: string; 'aria-hidden'?: boolean | 'true' | 'false' }>;
	}[] = [
		{ value: 'light', get label() { return t('settings.light2') }, icon: Sun },
		{ value: 'dark', get label() { return t('settings.dark') }, icon: Moon },
		{ value: 'aligned', get label() { return t('settings.aligned') }, icon: SunMoon }
	];

	/** What each colour changes, shown as a sample on its preview. */
	const SAMPLES: Record<ColorRole, string> = {
		get accent() { return t('settings.primaryAction') },
		get background() { return t('settings.mainBackgroundText') },
		get panel() { return t('settings.panelText') }
	};

	const current = $derived(app.prefs.theme);
	/** The mode whose colours are being edited: the one on screen (system resolves to light or dark). */
	const mode = $derived<ResolvedMode>(theme.resolved);

	/** Applies the mode at once and saves it with the other preferences. */
	function choose(next: ThemeMode) {
		setTheme(next);
		app.setPrefs({ theme: next });
	}

	/** Stores one colour of the current mode (`null` = the theme token); the other mode is left as it is. */
	function setColor(role: ColorRole, hex: Hex | null) {
		const all = app.prefs.colors;
		const colors = { light: { ...all.light }, dark: { ...all.dark } };
		colors[mode][role] = hex;
		app.setPrefs({ colors });
	}
</script>

<div class="flex flex-col gap-6">
	<SettingsGroup title={t('settings.theme')}>
		<fieldset class="flex flex-col gap-0 p-0">
			<legend class="sr-only">{t('settings.theme')}</legend>
			<div class="grid gap-2 p-3 sm:grid-cols-3">
				{#each OPTIONS as option (option.value)}
					{@const Icon = option.icon}
					<label
						class="flex cursor-pointer items-center gap-2 rounded-md border border-border bg-background px-3 py-2 text-ui font-medium text-foreground transition-colors hover:bg-muted has-[:checked]:border-primary has-[:checked]:ring-1 has-[:checked]:ring-primary has-[:focus-visible]:ring-2 has-[:focus-visible]:ring-ring"
					>
						<input
							type="radio"
							name="theme-mode"
							value={option.value}
							class="sr-only"
							checked={current === option.value}
							onchange={() => choose(option.value)}
						/>
						<Icon class="size-4" aria-hidden="true" />
						{option.label}
					</label>
				{/each}
			</div>
		</fieldset>
	</SettingsGroup>

	<SettingsGroup title={t('settings.colors')}>
		{#each COLOR_ROLES as role (role)}
			<ColorChoice
				label={COLOR_ROLE_LABELS[role]}
				value={roleColor(app.prefs.colors[mode], mode, role)}
				custom={isCustomRole(app.prefs.colors[mode], role)}
				foreground={roleForeground(app.prefs.colors[mode], mode, role)}
				sample={SAMPLES[role]}
				presets={PRESETS[role]}
				checks={colorChecks(app.prefs.colors[mode], mode, role)}
				onChange={(hex) => setColor(role, hex)}
				onReset={() => setColor(role, null)}
			/>
		{/each}
	</SettingsGroup>

	<SettingsGroup title={t('settings.appIcon')}>
		<fieldset class="flex flex-col gap-0 p-0">
			<legend class="sr-only">{t('settings.appIcon')}</legend>
			<div class="grid gap-2 p-3 sm:grid-cols-3">
				{#each ICON_OPTIONS as option (option.value)}
					{@const Icon = option.icon}
					<label
						class="flex cursor-pointer items-center gap-2 rounded-md border border-border bg-background px-3 py-2 text-ui font-medium text-foreground transition-colors hover:bg-muted has-[:checked]:border-primary has-[:checked]:ring-1 has-[:checked]:ring-primary has-[:focus-visible]:ring-2 has-[:focus-visible]:ring-ring"
					>
						<input
							type="radio"
							name="app-icon"
							value={option.value}
							class="sr-only"
							checked={app.prefs.app_icon === option.value}
							onchange={() => app.setPrefs({ app_icon: option.value })}
						/>
						<Icon class="size-4" aria-hidden="true" />
						{option.label}
					</label>
				{/each}
			</div>
		</fieldset>
	</SettingsGroup>
</div>
