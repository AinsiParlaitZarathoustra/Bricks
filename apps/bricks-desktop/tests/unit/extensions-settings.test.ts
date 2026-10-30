// @vitest-environment happy-dom
// Extensions (ExtensionsSettings.svelte), mounted in happy-dom with the Computer Use IPC functions
// replaced: the built-in row carries its badge and its switch; the switch saves the choice and tells
// the app; each permission reads granted, missing or unknown, with « Ouvrir les Réglages » only for a
// missing one; the switch is off while the driver is missing or unsupported; the state is read again
// when the window comes back to the front. The section sits in the settings, between Skills and Application.
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { afterEach, beforeEach, describe, expect, it, vi, type MockInstance } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import ExtensionsSettings from '../../src/lib/features/settings/ExtensionsSettings.svelte';
import * as ipc from '../../src/lib/ipc/client';
import type { ComputerUseStatus } from '../../src/lib/ipc/bindings/ComputerUseStatus';
import { DEFAULT_PREFERENCES, app } from '../../src/lib/state/app.svelte';

vi.mock('@tauri-apps/api/core', () => {
	class Channel {
		onmessage: ((event: unknown) => void) | null = null;
	}
	return { Channel, invoke: vi.fn() };
});
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }));
vi.mock('$lib/ipc/client', async (importOriginal) => ({
	...(await importOriginal<typeof import('../../src/lib/ipc/client')>()),
	computerUseStatus: vi.fn(),
	computerUseSetEnabled: vi.fn(),
	computerUseRequestPermission: vi.fn()
}));

// The npm scripts run from apps/bricks-desktop (happy-dom gives no file URL to resolve from).
const src = (path: string) => readFileSync(join(process.cwd(), 'src', path), 'utf8');

function status(over: Partial<ComputerUseStatus> = {}): ComputerUseStatus {
	return {
		enabled: false,
		state: 'off',
		error: null,
		tools: 0,
		driver: { path: '/Applications/CuaDriver.app', version: '1.2.0', bundled: true },
		accessibility: 'granted',
		screen_recording: 'granted',
		...over
	};
}

let target: HTMLDivElement;
let component: ReturnType<typeof mount> | null = null;
let noteSpy: MockInstance<typeof app.noteComputerUse>;

// Lets the pending IPC promises and the component's updates settle.
async function settle() {
	await new Promise((resolve) => setTimeout(resolve, 0));
	flushSync();
}

async function open(first: ComputerUseStatus = status()) {
	vi.mocked(ipc.computerUseStatus).mockResolvedValue(first);
	component = mount(ExtensionsSettings, { target });
	await settle();
}

function switchControl(): HTMLButtonElement {
	return target.querySelector<HTMLButtonElement>('button[role="switch"]')!;
}

function row(id: 'accessibility' | 'screen_recording'): HTMLElement | null {
	return target.querySelector<HTMLElement>(`[data-testid="permission-${id}"]`);
}

function buttonIn(element: Element | null, text: string): HTMLButtonElement | undefined {
	return [...(element?.querySelectorAll('button') ?? [])].find((b) => b.textContent?.includes(text));
}

beforeEach(() => {
	vi.mocked(ipc.computerUseStatus).mockReset();
	vi.mocked(ipc.computerUseSetEnabled).mockReset();
	vi.mocked(ipc.computerUseRequestPermission).mockReset();
	vi.mocked(ipc.computerUseRequestPermission).mockResolvedValue(undefined);
	app.prefs = { ...DEFAULT_PREFERENCES, computer_use_enabled: false };
	noteSpy = vi.spyOn(app, 'noteComputerUse');
	target = document.createElement('div');
	document.body.appendChild(target);
});

afterEach(() => {
	if (component) void unmount(component);
	component = null;
	noteSpy.mockRestore();
	target.remove();
});

describe('the Computer Use row', () => {
	it('is a built-in MCP server: its name, the « Intégré » badge and one switch', async () => {
		await open();
		expect(target.textContent).toContain('Serveurs MCP');
		expect(target.textContent).toContain('Computer Use');
		expect(target.textContent).toContain('Intégré');
		expect(target.querySelectorAll('button[role="switch"]')).toHaveLength(1);
		const labelId = switchControl().getAttribute('aria-labelledby');
		expect(labelId).toBe('computer-use-label');
		expect(target.querySelector(`#${labelId}`)?.textContent).toBe('Computer Use');
	});

	it('shows the state off, and the switch off, when the choice is off', async () => {
		await open(status({ enabled: false, state: 'off' }));
		expect(target.querySelector('[data-testid="computer-use-state"]')?.textContent?.trim()).toBe('Désactivé');
		expect(switchControl().getAttribute('aria-checked')).toBe('false');
		expect(switchControl().disabled).toBe(false);
	});

	it('names the driver found, with its version and its path', async () => {
		await open(status());
		expect(target.textContent).toContain('Cua Driver 1.2.0');
		expect(target.textContent).toContain('/Applications/CuaDriver.app');
	});

	it('shows the driver error when it did not start', async () => {
		await open(status({ state: 'error', error: 'le pilote ne répond pas' }));
		expect(target.querySelector('[data-testid="computer-use-state"]')?.textContent?.trim()).toBe('Erreur');
		expect(target.querySelector('[role="alert"]')?.textContent?.trim()).toBe('le pilote ne répond pas');
	});

	it('turning it on saves the choice and tells the app, which keeps it', async () => {
		await open(status({ enabled: false, state: 'off' }));
		vi.mocked(ipc.computerUseSetEnabled).mockResolvedValue(status({ enabled: true, state: 'ready', tools: 2 }));

		switchControl().click();
		await settle();

		expect(ipc.computerUseSetEnabled).toHaveBeenCalledTimes(1);
		expect(ipc.computerUseSetEnabled).toHaveBeenCalledWith(true);
		expect(noteSpy).toHaveBeenCalledWith(true);
		expect(app.prefs.computer_use_enabled).toBe(true);
		expect(target.querySelector('[data-testid="computer-use-state"]')?.textContent?.trim()).toBe('Actif · 2 outils');
	});

	it('a refused change shows its reason and tells the app nothing', async () => {
		await open(status({ enabled: false, state: 'off' }));
		vi.mocked(ipc.computerUseSetEnabled).mockRejectedValue(new Error('refusé par le système'));

		switchControl().click();
		await settle();

		expect(ipc.computerUseSetEnabled).toHaveBeenCalledWith(true);
		expect(noteSpy).not.toHaveBeenCalled();
		expect(target.querySelector('[role="alert"]')?.textContent).toContain('refusé par le système');
	});

	it('is disabled while the driver is missing', async () => {
		await open(status({ state: 'missing', driver: null }));
		expect(target.querySelector('[data-testid="computer-use-state"]')?.textContent?.trim()).toBe('Cua Driver introuvable');
		expect(switchControl().disabled).toBe(true);
	});

	it('is disabled on a system that does not support it', async () => {
		await open(status({ state: 'unsupported', driver: null }));
		expect(target.querySelector('[data-testid="computer-use-state"]')?.textContent?.trim()).toBe('Indisponible sur ce système');
		expect(switchControl().disabled).toBe(true);
	});
});

describe('the permissions of the driver', () => {
	it('shows each one as granted, missing or unknown, and none needs a request when granted or unknown', async () => {
		await open(status({ state: 'ready', enabled: true, accessibility: 'granted', screen_recording: 'unknown' }));
		expect(row('accessibility')?.textContent).toContain('Accessibilité');
		expect(row('accessibility')?.textContent).toContain('Autorisée');
		expect(row('screen_recording')?.textContent).toContain('Capture d’écran');
		expect(row('screen_recording')?.textContent).toContain('Non vérifiée');
		expect(target.textContent).not.toContain('Ouvrir les Réglages');
	});

	it('shows a missing permission as not granted, with a button to open the Réglages', async () => {
		await open(status({ state: 'ready', enabled: true, accessibility: 'missing', screen_recording: 'granted' }));
		expect(row('accessibility')?.textContent).toContain('Non autorisée');
		expect(row('screen_recording')?.textContent).toContain('Autorisée');
		expect(buttonIn(row('accessibility'), 'Ouvrir les Réglages')).toBeDefined();
		expect(buttonIn(row('screen_recording'), 'Ouvrir les Réglages')).toBeUndefined();
	});

	it('opens the Réglages for the permission chosen', async () => {
		await open(status({ state: 'ready', enabled: true, accessibility: 'granted', screen_recording: 'missing' }));
		buttonIn(row('screen_recording'), 'Ouvrir les Réglages')!.click();
		await settle();
		expect(ipc.computerUseRequestPermission).toHaveBeenCalledTimes(1);
		expect(ipc.computerUseRequestPermission).toHaveBeenCalledWith('screen_recording');
	});

	it('the permissions are not listed while the driver is missing', async () => {
		await open(status({ state: 'missing', driver: null, accessibility: 'missing', screen_recording: 'missing' }));
		expect(row('accessibility')).toBeNull();
		expect(row('screen_recording')).toBeNull();
		expect(target.textContent).not.toContain('Ouvrir les Réglages');
	});
});

describe('the grants read again when the window comes back', () => {
	it('reads the status once on mount, and again on each focus of the window', async () => {
		await open(status({ state: 'ready', enabled: true, accessibility: 'missing' }));
		expect(ipc.computerUseStatus).toHaveBeenCalledTimes(1);

		vi.mocked(ipc.computerUseStatus).mockResolvedValue(status({ state: 'ready', enabled: true, accessibility: 'granted' }));
		window.dispatchEvent(new Event('focus'));
		await settle();

		expect(ipc.computerUseStatus).toHaveBeenCalledTimes(2);
		expect(row('accessibility')?.textContent).toContain('Autorisée');
		expect(buttonIn(row('accessibility'), 'Ouvrir les Réglages')).toBeUndefined();
	});

	it('stops reading it once unmounted', async () => {
		await open(status());
		const calls = vi.mocked(ipc.computerUseStatus).mock.calls.length;
		if (component) void unmount(component);
		component = null;
		window.dispatchEvent(new Event('focus'));
		await settle();
		expect(vi.mocked(ipc.computerUseStatus).mock.calls.length).toBe(calls);
	});
});

describe('the Extensions section of the settings', () => {
	const dialog = src('lib/features/settings/SettingsDialog.svelte');

	it('is a section named Extensions, between Skills and Application, showing ExtensionsSettings', () => {
		expect(dialog).toContain("{ id: 'extensions', label: 'Extensions', icon: Plug }");
		const sections = dialog.slice(dialog.indexOf('const SECTIONS'), dialog.indexOf('];', dialog.indexOf('const SECTIONS')));
		expect(sections.indexOf("id: 'skills'")).toBeLessThan(sections.indexOf("id: 'extensions'"));
		expect(sections.indexOf("id: 'extensions'")).toBeLessThan(sections.indexOf("id: 'application'"));
		expect(dialog).toContain('<Tabs.Content value="extensions"');
		expect(dialog).toContain('<ExtensionsSettings />');
	});
});
