<script lang="ts">
	// The MCP servers of the person (~/.bricks/mcp.toml, shared with `bricks mcp`) and those the
	// project in view declares in its bricks.toml: a project's server connects only once allowed,
	// as declared. Changes reach the conversations opened afterwards.
	import { t } from '$lib/i18n/i18n.svelte';
	import * as ipc from '$lib/ipc/client';
	import type { McpServerItem, McpServerInput, McpServersView, McpTestResult } from '$lib/ipc/client';
	import { app } from '$lib/state/app.svelte';
	import { Button } from '$lib/components/ui/button';
	import { Spinner } from '$lib/components/ui/spinner';
	import { Switch } from '$lib/components/ui/switch';
	import Plus from '@lucide/svelte/icons/plus';
	import Terminal from '@lucide/svelte/icons/terminal';
	import Globe from '@lucide/svelte/icons/globe';
	import Trash from '@lucide/svelte/icons/trash';
	import PlugZap from '@lucide/svelte/icons/plug-zap';
	import CircleCheck from '@lucide/svelte/icons/circle-check';
	import CircleX from '@lucide/svelte/icons/circle-x';
	import SettingsGroup from './SettingsGroup.svelte';
	import McpServerDialog from './McpServerDialog.svelte';
	import McpAppDialog from './McpAppDialog.svelte';
	import AppWindow from '@lucide/svelte/icons/app-window';

	const projectId = $derived(app.currentProject && !app.currentProject.chats ? app.currentProject.id : null);
	let list = $state<McpServersView | null>(null);
	let error = $state<string | null>(null);
	let adding = $state(false);
	/** The interface shown (MCP Apps prototype). */
	let app_ = $state<{ server: string; tool: string } | null>(null);
	/** Test results by server name; `null` while the test runs. */
	let tests = $state<Record<string, McpTestResult | null>>({});

	const mine = $derived(list?.servers.filter((s) => s.source === 'user') ?? []);
	const theirs = $derived(list?.servers.filter((s) => s.source === 'project') ?? []);

	async function run(action: () => Promise<McpServersView>) {
		try {
			list = await action();
			error = null;
		} catch (e) {
			error = ipc.errorMessage(e);
		}
	}

	async function add(input: McpServerInput): Promise<string | null> {
		try {
			list = await ipc.mcpAdd(input, projectId);
			return null;
		} catch (e) {
			return ipc.errorMessage(e);
		}
	}

	async function test(server: McpServerItem) {
		tests = { ...tests, [server.name]: null };
		try {
			tests = { ...tests, [server.name]: await ipc.mcpTest(server.name, projectId) };
		} catch (e) {
			tests = { ...tests, [server.name]: { ok: false, tools: [], apps: [], error: ipc.errorMessage(e) } };
		}
	}

	// The servers, again when another project comes into view.
	$effect(() => {
		const id = projectId;
		void run(() => ipc.mcpServers(id));
	});
</script>

{#snippet row(server: McpServerItem)}
	{@const result = tests[server.name]}
	{@const Icon = server.transport === 'http' ? Globe : Terminal}
	<div class="flex flex-col gap-1.5 p-3" data-testid="mcp-server-{server.name}">
		<div class="flex items-center gap-3">
			<Icon class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
			<div class="flex min-w-0 flex-1 flex-col gap-0.5">
				<span class="truncate text-ui font-medium text-foreground">{server.name}</span>
				<span class="truncate font-mono text-caption text-muted-foreground" title={server.target}>{server.target}</span>
			</div>
			{#if server.source === 'project' && server.decision !== 'approved'}
				{#if server.shadowed}
					<span class="text-caption text-muted-foreground">{t('settings.mcpShadowed')}</span>
				{:else}
					<span class="text-caption text-muted-foreground">
						{server.decision === 'denied' ? t('settings.mcpDenied') : t('settings.mcpPending')}
					</span>
					<Button size="sm" variant="secondary" onclick={() => projectId && void run(() => ipc.mcpDecide(projectId, server.name, true))}>
						{t('settings.mcpApprove')}
					</Button>
					{#if server.decision === 'pending'}
						<Button size="sm" variant="ghost" onclick={() => projectId && void run(() => ipc.mcpDecide(projectId, server.name, false))}>
							{t('settings.mcpDeny')}
						</Button>
					{/if}
				{/if}
			{:else}
				<Button
					size="icon-sm"
					variant="ghost"
					class="text-muted-foreground hover:text-foreground"
					disabled={result === null}
					aria-label={t('settings.mcpTest', { name: server.name })}
					title={t('settings.mcpTest', { name: server.name })}
					onclick={() => void test(server)}
				>
					{#if result === null}
						<Spinner aria-hidden="true" />
					{:else}
						<PlugZap aria-hidden="true" />
					{/if}
				</Button>
				{#if server.source === 'user'}
					<Button
						size="icon-sm"
						variant="ghost"
						class="text-muted-foreground hover:text-destructive"
						aria-label={t('settings.mcpRemove', { name: server.name })}
						title={t('settings.mcpRemove', { name: server.name })}
						onclick={() => void run(() => ipc.mcpRemove(server.name, projectId))}
					>
						<Trash aria-hidden="true" />
					</Button>
					<Switch
						aria-label={t('settings.mcpEnabled', { name: server.name })}
						checked={server.enabled}
						onCheckedChange={(on) => void run(() => ipc.mcpSetEnabled(server.name, on, projectId))}
					/>
				{:else if projectId}
					<Button size="sm" variant="ghost" onclick={() => void run(() => ipc.mcpDecide(projectId, server.name, false))}>
						{t('settings.mcpDeny')}
					</Button>
				{/if}
			{/if}
		</div>
		{#if result}
			{#if result.ok}
				<p class="flex items-center gap-1.5 pl-7 text-caption text-success">
					<CircleCheck class="size-3.5 shrink-0" aria-hidden="true" />
					<span class="min-w-0 truncate">{result.tools.length === 1 ? t('settings.mcpOneTool') : t('settings.mcpTools', { count: result.tools.length })} · {result.tools.join(', ')}</span>
				</p>
				{#each result.apps as tool (tool)}
					<div class="pl-7">
						<Button size="sm" variant="secondary" onclick={() => (app_ = { server: server.name, tool })}>
							<AppWindow aria-hidden="true" data-icon="inline-start" />
							{t('settings.mcpOpenApp', { tool })}
						</Button>
					</div>
				{/each}
			{:else}
				<p role="alert" class="flex items-start gap-1.5 pl-7 text-caption text-pretty text-destructive">
					<CircleX class="mt-px size-3.5 shrink-0" aria-hidden="true" />
					<span>{result.error}</span>
				</p>
			{/if}
		{/if}
	</div>
{/snippet}

<SettingsGroup title={t('settings.mcpYours')}>
	{#snippet action()}
		<Button
			size="icon-xs"
			variant="ghost"
			class="text-muted-foreground hover:text-foreground"
			aria-label={t('settings.mcpAdd')}
			title={t('settings.mcpAdd')}
			onclick={() => (adding = true)}
		>
			<Plus aria-hidden="true" />
		</Button>
	{/snippet}
	{#each mine as server (server.name)}
		{@render row(server)}
	{:else}
		<p class="p-3 text-ui text-muted-foreground">{t('settings.mcpNone')}</p>
	{/each}
</SettingsGroup>

{#if theirs.length > 0}
	<SettingsGroup title={t('settings.mcpProjectServers', { name: app.currentProject?.name ?? '' })}>
		{#each theirs as server (server.name)}
			{@render row(server)}
		{/each}
	</SettingsGroup>
{/if}

{#each [...(list?.errors ?? []), ...(error ? [error] : [])] as message (message)}
	<p role="alert" class="text-ui text-pretty text-destructive">{message}</p>
{/each}

{#if app_}
	<McpAppDialog server={app_.server} tool={app_.tool} {projectId} onClose={() => (app_ = null)} />
{/if}

{#if adding}
	<McpServerDialog onSave={add} onClose={() => (adding = false)} />
{/if}
