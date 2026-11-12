<script lang="ts">
	// Mermaid and SVG viewer preferences (`Preferences.diagrams`).
	import { t } from '$lib/i18n/i18n.svelte';
	import { app } from '$lib/state/app.svelte';
	import * as Select from '$lib/components/ui/select';
	import * as Field from '$lib/components/ui/field';
	import { Switch } from '$lib/components/ui/switch';
	import {
		MERMAID_LOOKS,
		MERMAID_SECURITY_LEVELS,
		MERMAID_THEMES,
		diagramSettings,
		diagramTheme,
		type MermaidLook,
		type MermaidSecurityLevel,
		type MermaidTheme
	} from '$lib/features/diagrams/settings';
	import type { DiagramViewerSettings } from '$lib/ipc/bindings/DiagramViewerSettings';
	import SettingsGroup from './SettingsGroup.svelte';

	const uid = $props.id();
	const settings = $derived(diagramSettings(app.prefs.diagrams));

	const SECURITY_LABEL: Record<MermaidSecurityLevel, string> = {
		sandbox: 'Sandbox',
		strict: 'Strict',
		antiscript: 'Antiscript',
		loose: 'Loose'
	};
	const THEME_LABEL: Record<'app' | MermaidTheme, string> = {
		get app() { return t('settings.bricksTheme') },
		get default() { return t('settings.mermaidThemeDefault') },
		get forest() { return t('settings.mermaidThemeForest') },
		get dark() { return t('settings.mermaidThemeDark') },
		get neutral() { return t('settings.mermaidThemeNeutral') },
		get base() { return t('settings.mermaidThemeBase') }
	};
	const LOOK_LABEL: Record<MermaidLook, string> = {
		get classic() { return t('settings.mermaidLookClassic') },
		get handDrawn() { return t('settings.mermaidLookHandDrawn') },
		get neo() { return t('settings.mermaidLookNeo') }
	};
	const THEMES = ['app', ...MERMAID_THEMES] as const;

	type Control = Exclude<keyof DiagramViewerSettings, 'followAppTheme' | 'capabilitiesRevision'>;
	const controls: { key: Control; label: string }[] = [
		{ key: 'allowZoom', get label() { return t('settings.allowZoom') } },
		{ key: 'allowPan', get label() { return t('settings.pan') } },
		{ key: 'allowFullscreen', get label() { return t('common.fullscreen') } },
		{ key: 'allowDownload', get label() { return t('settings.svgDownload') } },
		{ key: 'allowCopy', get label() { return t('settings.copySource') } },
		{ key: 'allowTooltips', get label() { return t('settings.tooltips') } },
		{ key: 'allowSelection', get label() { return t('settings.textSelection') } },
		{ key: 'allowLinks', get label() { return t('settings.links') } },
		{ key: 'allowAnimations', get label() { return t('settings.animations') } }
	];

	const theme = $derived(diagramTheme(settings));

	function chooseSecurity(next: string | undefined) {
		const level = MERMAID_SECURITY_LEVELS.find((value) => value === next);
		if (!level || level === settings.mermaid.securityLevel) return;
		app.setPrefs({ diagrams: { ...settings, mermaid: { ...settings.mermaid, securityLevel: level } } });
	}
	function chooseTheme(next: string | undefined) {
		if (next === 'app') {
			app.setPrefs({ diagrams: { ...settings, diagramViewer: { ...settings.diagramViewer, followAppTheme: true } } });
			return;
		}
		const chosen = MERMAID_THEMES.find((value) => value === next);
		if (!chosen) return;
		app.setPrefs({
			diagrams: {
				mermaid: { ...settings.mermaid, theme: chosen },
				diagramViewer: { ...settings.diagramViewer, followAppTheme: false }
			}
		});
	}
	function chooseLook(next: string | undefined) {
		const look = MERMAID_LOOKS.find((value) => value === next);
		if (!look || look === settings.mermaid.look) return;
		app.setPrefs({ diagrams: { ...settings, mermaid: { ...settings.mermaid, look } } });
	}
	function toggle(key: Control, value: boolean) {
		app.setPrefs({ diagrams: { ...settings, diagramViewer: { ...settings.diagramViewer, [key]: value } } });
	}
</script>

<Field.Field class="gap-3">
	<Field.Label for={`${uid}-security`} class="text-ui font-semibold text-foreground">{t('settings.mermaidSecurity')}</Field.Label>
	<Select.Root type="single" value={settings.mermaid.securityLevel} onValueChange={chooseSecurity}>
		<Select.Trigger id={`${uid}-security`} class="w-full max-w-72">
			<span class="truncate">{SECURITY_LABEL[settings.mermaid.securityLevel]}</span>
		</Select.Trigger>
		<Select.Content>
			{#each MERMAID_SECURITY_LEVELS as level (level)}
				<Select.Item value={level} label={SECURITY_LABEL[level]}>{SECURITY_LABEL[level]}</Select.Item>
			{/each}
		</Select.Content>
	</Select.Root>
</Field.Field>

<SettingsGroup title={t('settings.diagramsAndSvg')}>
	<div class="flex flex-wrap items-center gap-3 p-3 text-ui">
		<Field.Field orientation="horizontal" class="w-auto">
		<Field.Label for={`${uid}-theme`} class="text-ui">{t('settings.theme')}</Field.Label>
		<Select.Root type="single" value={theme} onValueChange={chooseTheme}>
			<Select.Trigger id={`${uid}-theme`} class="w-44">
				<span class="truncate">{THEME_LABEL[theme]}</span>
			</Select.Trigger>
			<Select.Content>
				{#each THEMES as value (value)}
					<Select.Item {value} label={THEME_LABEL[value]}>{THEME_LABEL[value]}</Select.Item>
				{/each}
			</Select.Content>
		</Select.Root>
		</Field.Field>
		<Field.Field orientation="horizontal" class="w-auto">
		<Field.Label for={`${uid}-look`} class="text-ui">{t('settings.style')}</Field.Label>
		<Select.Root type="single" value={settings.mermaid.look} onValueChange={chooseLook}>
			<Select.Trigger id={`${uid}-look`} class="w-36">
				<span class="truncate">{LOOK_LABEL[settings.mermaid.look]}</span>
			</Select.Trigger>
			<Select.Content>
				{#each MERMAID_LOOKS as look (look)}
					<Select.Item value={look} label={LOOK_LABEL[look]}>{LOOK_LABEL[look]}</Select.Item>
				{/each}
			</Select.Content>
		</Select.Root>
		</Field.Field>
	</div>
	{#each controls as control (control.key)}
		<label class="flex items-center justify-between gap-3 p-3 text-ui">
			{control.label}
			<Switch
				checked={settings.diagramViewer[control.key]}
				onCheckedChange={(on) => toggle(control.key, on)}
			/>
		</label>
	{/each}
</SettingsGroup>
