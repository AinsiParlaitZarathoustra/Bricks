<script lang="ts">
	// Adds a server of the person's: a command (stdio) or a URL (Streamable HTTP).
	import { t } from '$lib/i18n/i18n.svelte';
	import * as Dialog from '$lib/components/ui/dialog';
	import * as Tabs from '$lib/components/ui/tabs';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import * as Field from '$lib/components/ui/field';
	import { Textarea } from '$lib/components/ui/textarea';
	import type { McpServerInput } from '$lib/ipc/client';

	let {
		onSave,
		onClose
	}: {
		/** Resolves with the refusal, or `null` once added. */
		onSave: (input: McpServerInput) => Promise<string | null>;
		onClose: () => void;
	} = $props();

	const uid = $props.id();
	let name = $state('');
	let transport = $state<'stdio' | 'http'>('stdio');
	let command = $state('');
	let url = $state('');
	let pairs = $state('');
	let saving = $state(false);
	let error = $state<string | null>(null);

	const lines = (text: string) =>
		text
			.split('\n')
			.map((l) => l.trim())
			.filter((l) => l !== '');

	/** Field errors, shown once an add has been tried, then kept up to date. */
	let tried = $state(false);
	const errors = $derived.by(() => {
		const out: Partial<Record<'name' | 'command' | 'url' | 'pairs', string>> = {};
		if (name.trim() === '') out.name = t('settings.thisFieldIsRequired');
		if (transport === 'stdio' && command.trim() === '') out.command = t('settings.thisFieldIsRequired');
		if (transport === 'http') {
			const address = url.trim();
			if (address === '') out.url = t('settings.thisFieldIsRequired');
			else if (!/^https?:\/\/[^\s/]+/i.test(address)) out.url = t('settings.mcpUrlInvalid');
		}
		const bad = pairs.split('\n').findIndex((l) => l.trim() !== '' && !/^[^=\s]+=/.test(l.trim()));
		if (bad >= 0) out.pairs = t('settings.mcpPairInvalid', { line: bad + 1 });
		return out;
	});
	const shown = $derived(tried ? errors : {});

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		if (saving) return;
		tried = true;
		if (Object.keys(errors).length > 0) return;
		saving = true;
		error = null;
		error = await onSave({
			name: name.trim(),
			transport,
			command: transport === 'stdio' ? command : '',
			url: transport === 'http' ? url.trim() : '',
			headers: transport === 'http' ? lines(pairs) : [],
			env: transport === 'stdio' ? lines(pairs) : []
		});
		saving = false;
		if (error === null) onClose();
	}
</script>

<Dialog.Root open onOpenChange={(next) => (next ? undefined : onClose())}>
	<Dialog.Content class="sm:max-w-lg">
		<form onsubmit={submit} class="flex flex-col gap-5">
			<Dialog.Header>
				<Dialog.Title>{t('settings.mcpAdd')}</Dialog.Title>
			</Dialog.Header>

			<Field.Field data-invalid={shown.name ? 'true' : undefined}>
				<Field.Label for={`${uid}-name`}>{t('settings.name')}</Field.Label>
				<Input
					id={`${uid}-name`}
					bind:value={name}
					autocomplete="off"
					spellcheck={false}
					class="font-mono text-sm"
					aria-invalid={shown.name ? 'true' : undefined}
					aria-describedby={shown.name ? `${uid}-name-error` : undefined}
				/>
				{#if shown.name}
					<Field.Error id={`${uid}-name-error`}>{shown.name}</Field.Error>
				{/if}
			</Field.Field>

			<Tabs.Root bind:value={transport}>
				<Tabs.List class="w-full">
					<Tabs.Trigger value="stdio" class="flex-1">{t('settings.mcpCommand')}</Tabs.Trigger>
					<Tabs.Trigger value="http" class="flex-1">{t('settings.mcpUrl')}</Tabs.Trigger>
				</Tabs.List>
			</Tabs.Root>

			{#if transport === 'stdio'}
				<Field.Field data-invalid={shown.command ? 'true' : undefined}>
					<Field.Label for={`${uid}-command`}>{t('settings.mcpCommand')}</Field.Label>
					<Input
						aria-invalid={shown.command ? 'true' : undefined}
						aria-describedby={shown.command ? `${uid}-command-error` : undefined}
						id={`${uid}-command`}
						bind:value={command}
						placeholder="npx -y @modelcontextprotocol/server-filesystem ."
						autocomplete="off"
						spellcheck={false}
						class="font-mono text-sm"
					/>
					{#if shown.command}
						<Field.Error id={`${uid}-command-error`}>{shown.command}</Field.Error>
					{/if}
				</Field.Field>
			{:else}
				<Field.Field data-invalid={shown.url ? 'true' : undefined}>
					<Field.Label for={`${uid}-url`}>{t('settings.mcpUrl')}</Field.Label>
					<Input
						aria-invalid={shown.url ? 'true' : undefined}
						aria-describedby={shown.url ? `${uid}-url-error` : undefined}
						id={`${uid}-url`}
						bind:value={url}
						placeholder="https://example.com/mcp"
						autocomplete="off"
						spellcheck={false}
						class="font-mono text-sm"
					/>
					{#if shown.url}
						<Field.Error id={`${uid}-url-error`}>{shown.url}</Field.Error>
					{/if}
				</Field.Field>
			{/if}

			<Field.Field data-invalid={shown.pairs ? 'true' : undefined}>
				<Field.Label for={`${uid}-pairs`}>{transport === 'http' ? t('settings.mcpHeaders') : t('settings.mcpEnv')}</Field.Label>
				<Textarea
					aria-invalid={shown.pairs ? 'true' : undefined}
					aria-describedby={shown.pairs ? `${uid}-pairs-error` : undefined}
					id={`${uid}-pairs`}
					bind:value={pairs}
					rows={3}
					placeholder={transport === 'http' ? 'Authorization=Bearer ${TOKEN}' : 'NAME=value'}
					spellcheck={false}
					class="font-mono text-sm"
				/>
				{#if shown.pairs}
					<Field.Error id={`${uid}-pairs-error`}>{shown.pairs}</Field.Error>
				{/if}
			</Field.Field>

			{#if error}
				<p role="alert" class="text-sm text-pretty text-destructive">{error}</p>
			{/if}

			<Dialog.Footer>
				<Button type="button" variant="outline" onclick={onClose}>{t('common.cancel')}</Button>
				<Button type="submit" disabled={saving}>{t('common.add')}</Button>
			</Dialog.Footer>
		</form>
	</Dialog.Content>
</Dialog.Root>
