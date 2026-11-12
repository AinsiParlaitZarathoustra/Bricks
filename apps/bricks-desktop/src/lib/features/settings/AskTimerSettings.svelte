<script lang="ts">
	// The timer of the questions the model asks (`Ask`): off by default; SMART lets
	// the model choose each duration, CUSTOM imposes one. Saved in the preferences
	// and sent with each message: a change applies from the next message.
	import { t } from '$lib/i18n/i18n.svelte';
	import type { AskTimerMode } from '$lib/ipc/client';
	import * as Field from '$lib/components/ui/field';
	import { Input } from '$lib/components/ui/input';
	import { Switch } from '$lib/components/ui/switch';
	import { app } from '$lib/state/app.svelte';
	import SettingsGroup from './SettingsGroup.svelte';

	const MIN = 30;
	const MAX = 3600;
	const MODES: { value: AskTimerMode; label: string }[] = [
		{ value: 'smart', label: 'SMART' },
		{ value: 'custom', label: 'CUSTOM' }
	];

	const uid = $props.id();
	const timer = $derived(app.prefs.ask_timer);
	/** The duration as typed, kept apart until it is valid. */
	let typed = $state<string | null>(null);
	const seconds = $derived(typed ?? String(timer.seconds));
	const invalid = $derived.by(() => {
		const n = Number(seconds);
		return !Number.isInteger(n) || n < MIN || n > MAX;
	});

	function update(patch: Partial<typeof timer>) {
		app.setPrefs({ ask_timer: { ...timer, ...patch } });
	}

	function onSeconds(value: string) {
		typed = value;
		const n = Number(value);
		if (Number.isInteger(n) && n >= MIN && n <= MAX) update({ seconds: n });
	}
</script>

<SettingsGroup title={t('settings.questionTimer')}>
	<label class="flex items-center justify-between gap-3 p-3 text-ui">
		{t('settings.enableTheTimer')}
		<Switch checked={timer.enabled} onCheckedChange={(on) => update({ enabled: on })} />
	</label>
	{#if timer.enabled}
		<fieldset class="flex flex-wrap items-center gap-4 p-3 text-ui">
			<legend class="sr-only">{t('settings.timerMode')}</legend>
			{#each MODES as mode (mode.value)}
				<label class="flex items-center gap-2">
					<input
						type="radio"
						name={`${uid}-mode`}
						value={mode.value}
						checked={timer.mode === mode.value}
						onchange={() => update({ mode: mode.value })}
					/>
					{mode.label}
				</label>
			{/each}
			{#if timer.mode === 'custom'}
				<Field.Field orientation="horizontal" class="w-auto" data-invalid={invalid ? 'true' : undefined}>
					<Field.Label for={`${uid}-seconds`} class="text-ui">{t('settings.durationS')}</Field.Label>
					<Input
						id={`${uid}-seconds`}
						type="number"
						min={MIN}
						max={MAX}
						step={1}
						value={seconds}
						aria-invalid={invalid ? 'true' : undefined}
						aria-describedby={invalid ? `${uid}-error` : undefined}
						class="h-8 w-24 text-ui"
						oninput={(event) => onSeconds(event.currentTarget.value)}
					/>
				</Field.Field>
				{#if invalid}
					<Field.Error id={`${uid}-error`} class="w-full text-caption">
						{t('settings.betweenAndSeconds', { MIN, MAX })}
					</Field.Error>
				{/if}
			{/if}
		</fieldset>
	{/if}
</SettingsGroup>
