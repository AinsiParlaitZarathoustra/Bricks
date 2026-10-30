<script lang="ts">
	// Mermaid and SVG viewer preferences (`Preferences.diagrams`).
	import { app } from '$lib/state/app.svelte';
	import * as Select from '$lib/components/ui/select';
	import { Label } from '$lib/components/ui/label';
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
		app: 'Thème de Bricks',
		default: 'Default',
		forest: 'Forest',
		dark: 'Dark',
		neutral: 'Neutral',
		base: 'Base'
	};
	const LOOK_LABEL: Record<MermaidLook, string> = { classic: 'Classic', handDrawn: 'Hand drawn', neo: 'Neo' };
	const THEMES = ['app', ...MERMAID_THEMES] as const;

	const controls: { key: Exclude<keyof DiagramViewerSettings, 'followAppTheme'>; label: string }[] = [
		{ key: 'allowZoom', label: 'Zoom' },
		{ key: 'allowPan', label: 'Déplacement' },
		{ key: 'allowFullscreen', label: 'Plein écran' },
		{ key: 'allowDownload', label: 'Téléchargement SVG' },
		{ key: 'allowCopy', label: 'Copie de la source' },
		{ key: 'allowTooltips', label: 'Infobulles' },
		{ key: 'allowSelection', label: 'Sélection du texte' }
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
	function toggle(key: keyof DiagramViewerSettings, value: boolean) {
		app.setPrefs({ diagrams: { ...settings, diagramViewer: { ...settings.diagramViewer, [key]: value } } });
	}
</script>

<div class="flex flex-col gap-3">
	<Label for={`${uid}-security`} class="text-ui font-semibold text-foreground">Sécurité Mermaid</Label>
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
</div>

<SettingsGroup title="Diagrammes et SVG">
	<div class="flex flex-wrap items-center gap-3 p-3 text-ui">
		<Label for={`${uid}-theme`} class="text-ui">Thème</Label>
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
		<Label for={`${uid}-look`} class="text-ui">Style</Label>
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
