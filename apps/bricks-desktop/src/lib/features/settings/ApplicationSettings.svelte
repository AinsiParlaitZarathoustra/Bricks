<script lang="ts">
	import { t } from '$lib/i18n/i18n.svelte';
	import { app } from '$lib/state/app.svelte';
	import SettingsGroup from './SettingsGroup.svelte';
	import CopyValue from './CopyValue.svelte';
	import { platformLabel, versionLabel } from './format';

	const info = $derived(app.info);
</script>

<div class="flex flex-col gap-6">
	{#if !info}
		<p class="text-ui text-muted-foreground">{t('settings.informationUnavailable')}</p>
	{:else}
		<SettingsGroup title="Bricks">
			<dl class="grid grid-cols-[auto_1fr] items-center gap-x-6 gap-y-2 p-3 text-ui">
				<dt class="text-muted-foreground">{t('settings.name')}</dt>
				<dd class="text-foreground">Bricks</dd>
				<dt class="text-muted-foreground">{t('settings.version')}</dt>
				<dd class="tabular-nums text-foreground" title={info.version}>{versionLabel(info.version)}</dd>
				<dt class="text-muted-foreground">{t('settings.platform')}</dt>
				<dd class="text-foreground">{platformLabel(info.platform)}</dd>
			</dl>
		</SettingsGroup>

		<SettingsGroup title={t('settings.folders')}>
			<div class="flex flex-col gap-2 p-3">
				<span class="text-ui text-muted-foreground" id="settings-bricks-home-label">{t('settings.bricksFolder')}</span>
				<div role="group" aria-labelledby="settings-bricks-home-label">
					<CopyValue label={t('settings.bricksFolder')} value={info.bricks_home} />
				</div>
			</div>
			<div class="flex flex-col gap-2 p-3">
				<span class="text-ui text-muted-foreground" id="settings-sessions-label">{t('settings.conversationsFolder')}</span>
				<div role="group" aria-labelledby="settings-sessions-label">
					<CopyValue label={t('settings.conversationsFolder')} value={info.sessions_dir} />
				</div>
			</div>
		</SettingsGroup>
	{/if}
</div>
