<script lang="ts">
	// Prototype of an MCP App: the interface of a tool, shown in a frame that is isolated from
	// Bricks (its own null origin: no Tauri command, no access to this page, no storage, no
	// network), and that speaks only through the bridge below (JSON-RPC over postMessage). The
	// bridge forwards `tools/call` to the tools of the same server only. What the interface
	// reports about its sandbox (`ui/report`) is shown under it.
	import { t } from '$lib/i18n/i18n.svelte';
	import * as Dialog from '$lib/components/ui/dialog';
	import * as ipc from '$lib/ipc/client';

	let {
		server,
		tool,
		projectId,
		onClose
	}: { server: string; tool: string; projectId: string | null; onClose: () => void } = $props();

	let frame = $state<HTMLIFrameElement | null>(null);
	let html = $state<string | null>(null);
	let error = $state<string | null>(null);
	let report = $state<Record<string, string> | null>(null);
	let calls = $state<string[]>([]);

	/** The frame may load nothing from anywhere: only its own inline scripts and styles. */
	const CSP =
		"<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:\">";

	$effect(() => {
		let gone = false;
		ipc
			.mcpAppLoad(server, tool, projectId)
			.then((view) => {
				if (!gone) html = CSP + view.html;
			})
			.catch((e) => {
				if (!gone) error = ipc.errorMessage(e);
			});
		return () => {
			gone = true;
		};
	});

	function reply(id: unknown, body: { result?: unknown; error?: { code: number; message: string } }) {
		frame?.contentWindow?.postMessage({ jsonrpc: '2.0', id, ...body }, '*');
	}

	async function onMessage(event: MessageEvent) {
		// Only the frame's own messages: nothing else on this page speaks for it.
		if (!frame || event.source !== frame.contentWindow) return;
		const message = event.data as { id?: unknown; method?: string; params?: Record<string, unknown> };
		if (!message || typeof message !== 'object' || typeof message.method !== 'string') return;
		if (message.method === 'ui/report') {
			report = Object.fromEntries(Object.entries(message.params ?? {}).map(([k, v]) => [k, String(v)]));
			return;
		}
		if (message.method !== 'tools/call' || message.id === undefined) {
			if (message.id !== undefined) reply(message.id, { error: { code: -32601, message: 'method not available' } });
			return;
		}
		const name = String(message.params?.name ?? '');
		const args = (message.params?.arguments ?? {}) as Record<string, unknown>;
		try {
			const result = await ipc.mcpAppCall(server, name, args, projectId);
			calls = [...calls, `${name} → ${result.text}`];
			reply(message.id, { result: { content: [{ type: 'text', text: result.text }], isError: result.is_error } });
		} catch (e) {
			const text = ipc.errorMessage(e);
			calls = [...calls, `${name} → ${text}`];
			reply(message.id, { error: { code: -32001, message: text } });
		}
	}
</script>

<svelte:window onmessage={onMessage} />

<Dialog.Root open onOpenChange={(next) => (next ? undefined : onClose())}>
	<Dialog.Content class="flex max-h-[min(44rem,calc(100dvh-2rem))] flex-col gap-4 sm:max-w-2xl">
		<Dialog.Header>
			<Dialog.Title>{server} · {tool}</Dialog.Title>
		</Dialog.Header>
		{#if error}
			<p role="alert" class="text-sm text-pretty text-destructive">{error}</p>
		{:else if html}
			<iframe
				bind:this={frame}
				title={t('settings.mcpAppFrame', { tool })}
				sandbox="allow-scripts"
				referrerpolicy="no-referrer"
				srcdoc={html}
				class="h-80 w-full rounded-lg border border-border bg-white"
			></iframe>
		{/if}
		{#if report}
			<div class="flex flex-col gap-1">
				<span class="text-caption font-medium text-muted-foreground">{t('settings.mcpAppIsolation')}</span>
				<pre class="rounded-lg bg-muted p-2 font-mono text-caption whitespace-pre-wrap">{Object.entries(report)
						.map(([k, v]) => `${k}: ${v}`)
						.join('\n')}</pre>
			</div>
		{/if}
		{#if calls.length > 0}
			<div class="flex flex-col gap-1">
				<span class="text-caption font-medium text-muted-foreground">{t('settings.mcpAppCalls')}</span>
				<pre class="rounded-lg bg-muted p-2 font-mono text-caption whitespace-pre-wrap">{calls.join('\n')}</pre>
			</div>
		{/if}
	</Dialog.Content>
</Dialog.Root>
