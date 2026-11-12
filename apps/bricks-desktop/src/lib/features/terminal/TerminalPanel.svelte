<script lang="ts">
	// Bottom pane: interactive shells on a real PTY, one set per project.
	// The sessions live in terminals.svelte.ts (store and session rules in
	// terminal-store / terminal-session); this component only shows the active
	// one.
	import { t } from '$lib/i18n/i18n.svelte';
	import '@xterm/xterm/css/xterm.css';
	import { untrack } from 'svelte';
	import Plus from '@lucide/svelte/icons/plus';
	import RotateCw from '@lucide/svelte/icons/rotate-cw';
	import X from '@lucide/svelte/icons/x';
	import { Button } from '$lib/components/ui/button';
	import * as ToggleGroup from '$lib/components/ui/toggle-group';
	import EmptyState from '$lib/layout/EmptyState.svelte';
	import { app } from '$lib/state/app.svelte';
	import { theme } from '$lib/theme/theme.svelte';
	import { terminals } from './terminals.svelte';

	/** Hides the pane (its sessions keep running); owned by the shell's `terminalOpen`. */
	let { onHide }: { onHide?: () => void } = $props();

	let container = $state<HTMLDivElement | null>(null);

	const projectId = $derived(app.currentProjectId);
	const list = $derived(projectId ? terminals.list(projectId) : []);
	const activeId = $derived(projectId ? terminals.activeId(projectId) : null);
	const active = $derived(list.find((s) => s.id === activeId) ?? null);
	const activeName = $derived(active ? t('terminal.terminal', { value: list.indexOf(active) + 1 }) : t('common.terminal'));

	// The first terminal of a project opens by itself, once per application run.
	$effect(() => {
		const pid = projectId;
		if (!pid) return;
		untrack(() => terminals.seed(pid, app.current?.sessionId ?? null));
	});

	// Show the active terminal here, start its shell if needed, and keep its
	// size in step with the pane. The cleanup runs before the next terminal is
	// attached (tab switch, project switch) and when the pane closes: it takes
	// this terminal out of the container, without disposing it.
	$effect(() => {
		const session = active;
		const element = container;
		if (!session || !element) return;
		session.attach(element);
		untrack(() => {
			if (session.status === 'idle') session.start();
		});
		session.focus();
		const observer = new ResizeObserver(() => session.requestRefit());
		observer.observe(element);
		return () => {
			observer.disconnect();
			session.detach();
		};
	});

	// Terminal colours follow the theme. Read after the `.dark` class is set.
	$effect(() => {
		void theme.resolved;
		untrack(() => terminals.refreshTheme());
	});

	function select(id: string) {
		if (projectId) terminals.activate(projectId, id);
	}

	function close(id: string) {
		if (projectId) terminals.close(projectId, id);
	}

	function create() {
		if (projectId) terminals.create(projectId, app.current?.sessionId ?? null);
	}
</script>

<div class="flex h-full min-h-0 flex-col bg-code-bg">
	<div class="flex h-9 shrink-0 items-center gap-1 border-b border-border bg-background px-2">
		<ToggleGroup.Root
			type="single"
			spacing={1}
			aria-label={t('terminal.projectTerminals')}
			class="min-w-0 flex-1 overflow-x-auto"
			bind:value={() => activeId ?? '', (id) => id && select(id)}
		>
			{#each list as session, index (session.id)}
				<div class="flex shrink-0 items-center rounded-md has-data-[state=on]:bg-element-selected">
					<ToggleGroup.Item
						value={session.id}
						class="h-6 min-w-0 px-2 text-xs font-normal hover:bg-transparent data-[state=on]:bg-transparent"
					>
						{t('terminal.terminal', { value: index + 1 })}
					</ToggleGroup.Item>
					<button
						type="button"
						class="rounded-md p-1 text-muted-foreground outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
						aria-label={t('terminal.closeTerminal', { value: index + 1 })}
						title={t('terminal.closeTheTerminal')}
						onclick={() => close(session.id)}
					>
						<X class="size-3.5" aria-hidden="true" />
					</button>
				</div>
			{/each}
		</ToggleGroup.Root>
		<Button
			variant="ghost"
			size="icon-sm"
			aria-label={t('terminal.newTerminal')}
			title={t('terminal.newTerminal')}
			disabled={!projectId}
			onclick={create}
		>
			<Plus aria-hidden="true" />
		</Button>
		{#if onHide}
			<Button
				variant="ghost"
				size="icon-sm"
				aria-label={t('common.hideTheTerminal')}
				title={t('common.hideTheTerminal')}
				onclick={onHide}
			>
				<X aria-hidden="true" />
			</Button>
		{/if}
	</div>

	{#if active?.cwd}
		<p class="truncate px-3 pt-1 font-mono text-xs text-muted-foreground" title={active.cwd}>{active.cwd}</p>
	{/if}

	<div class="relative min-h-0 flex-1">
		<div bind:this={container} role="group" aria-label={activeName} class="absolute inset-0 px-2 py-1"></div>

		{#if !active}
			<div class="absolute inset-0 flex items-center justify-center bg-code-bg">
				<EmptyState
					title={projectId ? t('terminal.noTerminalOpen') : t('common.noOpenProject')}
					description={projectId ? t('terminal.openATerminalInTheProject') : t('terminal.chooseAProjectToOpenA')}
				>
					{#snippet action()}
						{#if projectId}
							<Button variant="outline" size="sm" onclick={create}>{t('terminal.newTerminal')}</Button>
						{/if}
					{/snippet}
				</EmptyState>
			</div>
		{:else if active.status === 'exited'}
			<div
				role="status"
				class="absolute inset-x-0 bottom-0 flex items-center justify-between gap-2 border-t border-border bg-card px-3 py-2 text-xs text-card-foreground"
			>
				<span>
					{t('terminal.processFinished', { value: active.exitCode !== null ? t('terminal.code', { exitCode: active.exitCode }) : '' })}
				</span>
				<Button variant="outline" size="sm" onclick={() => active?.restart()}>
					<RotateCw aria-hidden="true" data-icon="inline-start" />
					{t('terminal.restart')}
				</Button>
			</div>
		{:else if active.status === 'error'}
			<div
				role="alert"
				class="absolute inset-x-0 bottom-0 flex items-center justify-between gap-2 border-t border-border bg-card px-3 py-2 text-xs text-destructive"
			>
				<span class="min-w-0 truncate">{t('terminal.couldNotStartTheTerminal', { error: active.error })}</span>
				<Button variant="outline" size="sm" onclick={() => active?.restart()}>{t('common.retry')}</Button>
			</div>
		{/if}
	</div>
</div>
