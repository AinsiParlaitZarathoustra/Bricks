<script lang="ts">
	import { app, DEFAULT_PREFERENCES } from '$lib/state/app.svelte';
	import { Switch } from '$lib/components/ui/switch';
	import { Label } from '$lib/components/ui/label';
	import { Button } from '$lib/components/ui/button';
	import SettingsGroup from './SettingsGroup.svelte';
	import { formatCount } from './format';

	let resetDone = $state(false);

	/** Layout keys reset to the shipped defaults. Theme and last-opened items are not layout. */
	function resetLayout() {
		app.setPrefs({
			left_open: DEFAULT_PREFERENCES.left_open,
			right_open: DEFAULT_PREFERENCES.right_open,
			terminal_open: DEFAULT_PREFERENCES.terminal_open,
			left_width: DEFAULT_PREFERENCES.left_width,
			right_width: DEFAULT_PREFERENCES.right_width,
			terminal_height: DEFAULT_PREFERENCES.terminal_height
		});
		resetDone = true;
	}
</script>

<div class="flex flex-col gap-6">
	<p class="text-sm text-pretty text-muted-foreground">
		Ces réglages sont enregistrés immédiatement et s’appliquent au prochain lancement de Bricks.
	</p>

	<SettingsGroup title="Panneaux à l’ouverture" description="État de chaque panneau quand Bricks démarre.">
		<div class="flex items-center justify-between gap-4 p-3">
			<Label for="pref-left-open" class="font-normal">Panneau de gauche ouvert</Label>
			<Switch
				id="pref-left-open"
				checked={app.prefs.left_open}
				onCheckedChange={(checked) => app.setPrefs({ left_open: checked })}
			/>
		</div>
		<div class="flex items-center justify-between gap-4 p-3">
			<Label for="pref-right-open" class="font-normal">Panneau de droite ouvert</Label>
			<Switch
				id="pref-right-open"
				checked={app.prefs.right_open}
				onCheckedChange={(checked) => app.setPrefs({ right_open: checked })}
			/>
		</div>
		<div class="flex items-center justify-between gap-4 p-3">
			<Label for="pref-terminal-open" class="font-normal">Terminal ouvert</Label>
			<Switch
				id="pref-terminal-open"
				checked={app.prefs.terminal_open}
				onCheckedChange={(checked) => app.setPrefs({ terminal_open: checked })}
			/>
		</div>
	</SettingsGroup>

	<SettingsGroup
		title="Dimensions"
		description="Mesurées quand vous faites glisser les séparateurs dans l’application. Lecture seule ici."
	>
		<dl class="grid grid-cols-[auto_1fr] items-center gap-x-6 gap-y-2 p-3 text-sm">
			<dt class="text-muted-foreground">Largeur du panneau de gauche</dt>
			<dd class="font-mono tabular-nums text-foreground">{formatCount(app.prefs.left_width)} px</dd>
			<dt class="text-muted-foreground">Largeur du panneau de droite</dt>
			<dd class="font-mono tabular-nums text-foreground">{formatCount(app.prefs.right_width)} px</dd>
			<dt class="text-muted-foreground">Hauteur du terminal</dt>
			<dd class="font-mono tabular-nums text-foreground">{formatCount(app.prefs.terminal_height)} px</dd>
		</dl>
		<div class="flex flex-col gap-2 p-3 sm:flex-row sm:items-center sm:justify-between">
			<p class="text-xs text-muted-foreground" role="status" aria-live="polite">
				{#if resetDone}
					Disposition réinitialisée. Elle s’appliquera au prochain lancement de Bricks.
				{/if}
			</p>
			<Button variant="outline" size="sm" onclick={resetLayout}>Réinitialiser la disposition</Button>
		</div>
	</SettingsGroup>
</div>
