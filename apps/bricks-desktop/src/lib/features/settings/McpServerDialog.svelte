<script lang="ts">
	// Adds a server of the person's: a command (stdio) or a URL (Streamable HTTP).
	import { t } from '$lib/i18n/i18n.svelte';
	import * as Dialog from '$lib/components/ui/dialog';
	import * as Tabs from '$lib/components/ui/tabs';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Label } from '$lib/components/ui/label';
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

	async function submit(event: SubmitEvent) {
		event.preventDefault();
		if (saving) return;
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

			<div class="flex flex-col gap-2">
				<Label for={`${uid}-name`}>{t('settings.name')}</Label>
				<Input id={`${uid}-name`} bind:value={name} autocomplete="off" spellcheck={false} class="font-mono text-sm" />
			</div>

			<Tabs.Root bind:value={transport}>
				<Tabs.List class="w-full">
					<Tabs.Trigger value="stdio" class="flex-1">{t('settings.mcpCommand')}</Tabs.Trigger>
					<Tabs.Trigger value="http" class="flex-1">{t('settings.mcpUrl')}</Tabs.Trigger>
				</Tabs.List>
			</Tabs.Root>

			{#if transport === 'stdio'}
				<div class="flex flex-col gap-2">
					<Label for={`${uid}-command`}>{t('settings.mcpCommand')}</Label>
					<Input
						id={`${uid}-command`}
						bind:value={command}
						placeholder="npx -y @modelcontextprotocol/server-filesystem ."
						autocomplete="off"
						spellcheck={false}
						class="font-mono text-sm"
					/>
				</div>
			{:else}
				<div class="flex flex-col gap-2">
					<Label for={`${uid}-url`}>{t('settings.mcpUrl')}</Label>
					<Input
						id={`${uid}-url`}
						bind:value={url}
						placeholder="https://example.com/mcp"
						autocomplete="off"
						spellcheck={false}
						class="font-mono text-sm"
					/>
				</div>
			{/if}

			<div class="flex flex-col gap-2">
				<Label for={`${uid}-pairs`}>{transport === 'http' ? t('settings.mcpHeaders') : t('settings.mcpEnv')}</Label>
				<Textarea
					id={`${uid}-pairs`}
					bind:value={pairs}
					rows={3}
					placeholder={transport === 'http' ? 'Authorization=Bearer ${TOKEN}' : 'NAME=value'}
					spellcheck={false}
					class="font-mono text-sm"
				/>
			</div>

			{#if error}
				<p role="alert" class="text-sm text-pretty text-destructive">{error}</p>
			{/if}

			<Dialog.Footer>
				<Button type="button" variant="outline" onclick={onClose}>{t('common.cancel')}</Button>
				<Button type="submit" disabled={saving || name.trim() === ''}>{t('common.add')}</Button>
			</Dialog.Footer>
		</form>
	</Dialog.Content>
</Dialog.Root>
