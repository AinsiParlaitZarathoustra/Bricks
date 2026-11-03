<script lang="ts">
	// The permission policy new conversations start from (`~/.bricks/permissions.json`).
	// Open conversations keep theirs.
	import { LANGUAGES, isLanguage, setLanguage, t } from '$lib/i18n/i18n.svelte';
	import * as Select from '$lib/components/ui/select';
	import { onMount } from 'svelte';
	import DiagramSettings from './DiagramSettings.svelte';
	import AskTimerSettings from './AskTimerSettings.svelte';
	import PolicyEditor from '$lib/features/permissions/PolicyEditor.svelte';
	import { unchosenPolicy } from '$lib/features/permissions/policy';
	import { errorMessage, permissionsDefaultGet, permissionsDefaultSet } from '$lib/ipc/client';
	import type { SessionPolicy } from '$lib/ipc/engine';
	import { Switch } from '$lib/components/ui/switch';
	import { app } from '$lib/state/app.svelte';

	let policy = $state<SessionPolicy | null>(null);
	let loadError = $state<string | null>(null);
	let saveError = $state<string | null>(null);
	let saving = $state(false);
	const languageLabel = $props.id();

	function chooseLanguage(value: string) {
		if (!isLanguage(value)) return;
		setLanguage(value);
		app.setPrefs({ language: value });
	}

	onMount(() => {
		void permissionsDefaultGet()
			.then((read) => {
				policy = read.policy ?? unchosenPolicy();
				loadError = read.error;
			})
			.catch((e) => {
				policy = unchosenPolicy();
				loadError = errorMessage(e);
			});
	});

	/** Saves the new default; the editor shows it only once it is saved. */
	async function save(next: SessionPolicy) {
		saving = true;
		saveError = null;
		try {
			await permissionsDefaultSet(next);
			policy = next;
			loadError = null;
		} catch (e) {
			saveError = errorMessage(e);
		} finally {
			saving = false;
		}
	}
</script>

<div class="flex flex-col gap-6">
	<div class="flex items-center justify-between gap-3">
		<span id={languageLabel} class="text-ui font-semibold text-foreground">{t('settings.language')}</span>
		<Select.Root type="single" value={app.prefs.language} onValueChange={chooseLanguage}>
			<Select.Trigger aria-labelledby={languageLabel} class="w-44">
				{LANGUAGES.find((l) => l.value === app.prefs.language)?.label}
			</Select.Trigger>
			<Select.Content>
				{#each LANGUAGES as { value, label } (value)}
					<Select.Item {value} {label}>{label}</Select.Item>
				{/each}
			</Select.Content>
		</Select.Root>
	</div>

	<section aria-labelledby="default-permissions" class="flex flex-col gap-3">
		<h3 id="default-permissions" class="text-ui font-semibold text-foreground">
			{t('settings.permissionsForNewConversations')}
		</h3>
		{#if policy}
			<PolicyEditor {policy} disabled={saving} onchange={save} />
		{/if}
		{#if loadError}
			<p role="alert" class="text-ui text-pretty text-destructive">
				{t('settings.couldNotRead', { loadError })}
			</p>
		{/if}
		{#if saveError}
			<p role="alert" class="text-ui text-pretty text-destructive">
				{t('settings.theSettingWasntSaved', { saveError })}
			</p>
		{/if}
	</section>

	<label class="flex items-center justify-between gap-3 text-ui font-semibold text-foreground">
		{t('common.keepTheComputerAwake')}
		<Switch checked={app.prefs.keep_awake} onCheckedChange={(on) => app.setPrefs({ keep_awake: on })} />
	</label>

	<DiagramSettings />
	<AskTimerSettings />
</div>
