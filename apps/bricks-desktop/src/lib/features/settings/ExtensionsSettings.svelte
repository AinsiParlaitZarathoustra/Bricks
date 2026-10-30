<script lang="ts">
	// Extensions: the MCP servers of Bricks. Computer Use (Cua Driver) is built in: it can be
	// turned on and off, never removed. Its macOS permissions are those of the driver's own
	// process, read again when the window comes back to the front.
	import { onMount } from 'svelte';
	import * as ipc from '$lib/ipc/client';
	import type { ComputerUseStatus } from '$lib/ipc/bindings/ComputerUseStatus';
	import type { ComputerUsePermission } from '$lib/ipc/bindings/ComputerUsePermission';
	import type { PermissionState } from '$lib/ipc/bindings/PermissionState';
	import { app } from '$lib/state/app.svelte';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import { Switch } from '$lib/components/ui/switch';
	import MousePointerClick from '@lucide/svelte/icons/mouse-pointer-click';
	import RefreshCw from '@lucide/svelte/icons/refresh-cw';
	import ExternalLink from '@lucide/svelte/icons/external-link';
	import SettingsGroup from './SettingsGroup.svelte';
	import { computerUseLabel, permissionLabel } from './extensions';

	let status = $state<ComputerUseStatus | null>(null);
	let loadError = $state<string | null>(null);
	let busy = $state(false);
	let checking = $state(false);
	let actionError = $state<string | null>(null);

	const PERMISSIONS: { id: ComputerUsePermission; label: string; key: 'accessibility' | 'screen_recording' }[] = [
		{ id: 'accessibility', label: 'Accessibilité', key: 'accessibility' },
		{ id: 'screen_recording', label: 'Capture d’écran', key: 'screen_recording' }
	];

	const available = $derived(status !== null && status.state !== 'unsupported' && status.state !== 'missing');
	const enabled = $derived(status?.enabled ?? app.prefs.computer_use_enabled);

	async function refresh() {
		checking = true;
		try {
			status = await ipc.computerUseStatus();
			loadError = null;
		} catch (e) {
			loadError = ipc.errorMessage(e);
		} finally {
			checking = false;
		}
	}

	async function setEnabled(on: boolean) {
		busy = true;
		actionError = null;
		try {
			status = await ipc.computerUseSetEnabled(on);
			app.noteComputerUse(status.enabled);
		} catch (e) {
			actionError = ipc.errorMessage(e);
		} finally {
			busy = false;
		}
	}

	async function request(permission: ComputerUsePermission) {
		actionError = null;
		try {
			await ipc.computerUseRequestPermission(permission);
		} catch (e) {
			actionError = ipc.errorMessage(e);
		}
	}

	function variant(state: PermissionState) {
		if (state === 'granted') return 'secondary';
		if (state === 'missing') return 'destructive';
		return 'outline';
	}

	onMount(() => {
		void refresh();
		// Back from System Settings: the grants are read again.
		const onFocus = () => void refresh();
		const onVisible = () => {
			if (document.visibilityState === 'visible') void refresh();
		};
		window.addEventListener('focus', onFocus);
		document.addEventListener('visibilitychange', onVisible);
		return () => {
			window.removeEventListener('focus', onFocus);
			document.removeEventListener('visibilitychange', onVisible);
		};
	});

	// While the driver starts, its state is read again every second.
	$effect(() => {
		if (status?.state !== 'starting') return;
		const timer = setTimeout(() => void refresh(), 1000);
		return () => clearTimeout(timer);
	});
</script>

<div class="flex flex-col gap-6">
	<SettingsGroup title="Serveurs MCP">
		<div class="flex items-center gap-3 p-3">
			<MousePointerClick class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
			<div class="flex min-w-0 flex-1 flex-col gap-0.5">
				<div class="flex flex-wrap items-center gap-2">
					<span id="computer-use-label" class="text-ui font-medium text-foreground">Computer Use</span>
					<Badge variant="outline">Intégré</Badge>
				</div>
				{#if status}
					<span class="text-caption text-muted-foreground" data-testid="computer-use-state">
						{computerUseLabel(status)}
					</span>
				{/if}
			</div>
			<Switch
				aria-labelledby="computer-use-label"
				checked={enabled}
				disabled={busy || !available}
				onCheckedChange={(on) => void setEnabled(on)}
			/>
		</div>
		{#if status?.driver}
			<div class="flex items-center justify-between gap-3 p-3 text-ui">
				<span class="shrink-0 whitespace-nowrap text-muted-foreground">Cua Driver{status.driver.version ? ` ${status.driver.version}` : ''}</span>
				<span class="min-w-0 truncate font-mono text-caption text-muted-foreground" title={status.driver.path}>
					{status.driver.path}
				</span>
			</div>
		{/if}
		{#if status?.state === 'error' && status.error}
			<p role="alert" class="p-3 text-ui text-pretty text-destructive">{status.error}</p>
		{/if}
		{#if loadError}
			<p role="alert" class="p-3 text-ui text-pretty text-destructive">{loadError}</p>
		{/if}
	</SettingsGroup>

	{#if available && status}
		<SettingsGroup title="Autorisations de Computer Use">
			{#each PERMISSIONS as permission (permission.id)}
				{@const state = status[permission.key]}
				<div class="flex items-center gap-3 p-3" data-testid="permission-{permission.id}">
					<span class="flex-1 text-ui text-foreground">{permission.label}</span>
					<Badge variant={variant(state)}>{permissionLabel(state)}</Badge>
					{#if state === 'missing'}
						<Button size="sm" variant="secondary" onclick={() => void request(permission.id)}>
							<ExternalLink class="size-3.5" aria-hidden="true" />
							Ouvrir les Réglages
						</Button>
					{/if}
				</div>
			{/each}
			<div class="flex justify-end p-3">
				<Button size="sm" variant="ghost" disabled={checking} onclick={() => void refresh()}>
					<RefreshCw class={['size-3.5', checking && 'animate-spin']} aria-hidden="true" />
					Revérifier
				</Button>
			</div>
		</SettingsGroup>
	{/if}
	{#if actionError}
		<p role="alert" class="text-ui text-pretty text-destructive">{actionError}</p>
	{/if}
</div>
