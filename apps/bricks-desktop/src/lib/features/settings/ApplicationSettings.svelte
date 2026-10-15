<script lang="ts">
	import { app } from '$lib/state/app.svelte';
	import SettingsGroup from './SettingsGroup.svelte';
	import CopyValue from './CopyValue.svelte';
	import { platformLabel } from './format';

	const info = $derived(app.info);
</script>

<div class="flex flex-col gap-6">
	{#if !info}
		<p class="text-sm text-muted-foreground">Informations indisponibles.</p>
	{:else}
		<SettingsGroup title="Bricks">
			<dl class="grid grid-cols-[auto_1fr] items-center gap-x-6 gap-y-2 p-3 text-sm">
				<dt class="text-muted-foreground">Nom</dt>
				<dd class="text-foreground">Bricks</dd>
				<dt class="text-muted-foreground">Version</dt>
				<dd class="font-mono tabular-nums text-foreground">{info.version}</dd>
				<dt class="text-muted-foreground">Plateforme</dt>
				<dd class="text-foreground">{platformLabel(info.platform)}</dd>
			</dl>
		</SettingsGroup>

		<SettingsGroup title="Dossiers" description="Où Bricks range sa configuration et ses conversations.">
			<div class="flex flex-col gap-2 p-3">
				<span class="text-sm text-muted-foreground" id="settings-bricks-home-label">Dossier Bricks</span>
				<div role="group" aria-labelledby="settings-bricks-home-label">
					<CopyValue label="Dossier Bricks" value={info.bricks_home} />
				</div>
			</div>
			<div class="flex flex-col gap-2 p-3">
				<span class="text-sm text-muted-foreground" id="settings-sessions-label">Dossier des conversations</span>
				<div role="group" aria-labelledby="settings-sessions-label">
					<CopyValue label="Dossier des conversations" value={info.sessions_dir} />
				</div>
			</div>
		</SettingsGroup>
	{/if}
</div>
