// @vitest-environment happy-dom
// « Délai des questions » (AskTimerSettings.svelte), mounted in happy-dom over the app store: off
// by default, SMART and CUSTOM once enabled, a duration kept between 30 and 3600 seconds (an
// invalid one is said and not saved), the chosen values kept while the timer is off, and the
// saved preference debounced. The section sits below the diagram settings in General.
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import AskTimerSettings from '../../src/lib/features/settings/AskTimerSettings.svelte';
import { DEFAULT_PREFERENCES, app } from '../../src/lib/state/app.svelte';

const ipc = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => {
	class Channel {
		onmessage: ((event: unknown) => void) | null = null;
	}
	return { Channel, invoke: (cmd: string, args?: unknown) => ipc.invoke(cmd, args) };
});
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }));

// The npm scripts run from apps/bricks-desktop (happy-dom gives no file URL to resolve from).
const src = (path: string) => readFileSync(join(process.cwd(), 'src', path), 'utf8');

let target: HTMLDivElement;
let component: ReturnType<typeof mount> | null = null;

beforeEach(() => {
	vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
	ipc.invoke.mockReset();
	ipc.invoke.mockResolvedValue(undefined);
	app.prefs = { ...DEFAULT_PREFERENCES, ask_timer: { enabled: false, mode: 'smart', seconds: 300 } };
	target = document.createElement('div');
	document.body.appendChild(target);
	component = mount(AskTimerSettings, { target });
	flushSync();
});

afterEach(() => {
	if (component) void unmount(component);
	component = null;
	target.remove();
	vi.useRealTimers();
});

function checkbox(): HTMLInputElement {
	return target.querySelector<HTMLInputElement>('input[type="checkbox"]')!;
}

function modes(): HTMLInputElement[] {
	return [...target.querySelectorAll<HTMLInputElement>('input[type="radio"]')];
}

function seconds(): HTMLInputElement | null {
	return target.querySelector<HTMLInputElement>('input[type="number"]');
}

function toggle(input: HTMLInputElement) {
	input.checked = !input.checked;
	input.dispatchEvent(new Event('change', { bubbles: true }));
	flushSync();
}

function pick(value: 'smart' | 'custom') {
	const radio = modes().find((r) => r.value === value)!;
	radio.checked = true;
	radio.dispatchEvent(new Event('change', { bubbles: true }));
	flushSync();
}

function type(value: string) {
	const input = seconds()!;
	input.value = value;
	input.dispatchEvent(new Event('input', { bubbles: true }));
	flushSync();
}

function alertText(): string | null {
	return target.querySelector('[role="alert"]')?.textContent?.trim() ?? null;
}

describe('« Délai des questions »', () => {
	it('is titled, and off by default: no mode and no duration to set', () => {
		expect(target.textContent).toContain('Délai des questions');
		expect(target.textContent).toContain('Activer le délai');
		expect(checkbox().checked).toBe(false);
		expect(modes()).toHaveLength(0);
		expect(seconds()).toBeNull();
	});

	it('offers SMART and CUSTOM once enabled, SMART first', () => {
		toggle(checkbox());
		expect(app.prefs.ask_timer.enabled).toBe(true);
		expect(modes().map((r) => r.value)).toEqual(['smart', 'custom']);
		expect(target.textContent).toContain('SMART');
		expect(target.textContent).toContain('CUSTOM');
		expect(modes()[0].checked).toBe(true);
		expect(seconds()).toBeNull();
	});

	it('CUSTOM asks for the duration in seconds, starting from the stored one', () => {
		toggle(checkbox());
		pick('custom');
		expect(app.prefs.ask_timer.mode).toBe('custom');
		expect(target.textContent).toContain('Durée (s)');
		expect(seconds()!.value).toBe('300');
	});

	it('keeps 30 and 3600 seconds, and says and refuses anything else', () => {
		toggle(checkbox());
		pick('custom');

		type('10');
		expect(seconds()!.getAttribute('aria-invalid')).toBe('true');
		expect(alertText()).toBe('Entre 30 et 3600 secondes.');
		expect(app.prefs.ask_timer.seconds).toBe(300);

		type('3601');
		expect(alertText()).toBe('Entre 30 et 3600 secondes.');
		expect(app.prefs.ask_timer.seconds).toBe(300);

		type('45.5');
		expect(alertText()).toBe('Entre 30 et 3600 secondes.');
		expect(app.prefs.ask_timer.seconds).toBe(300);

		type('30');
		expect(alertText()).toBeNull();
		expect(seconds()!.getAttribute('aria-invalid')).toBeNull();
		expect(app.prefs.ask_timer.seconds).toBe(30);

		type('3600');
		expect(alertText()).toBeNull();
		expect(app.prefs.ask_timer.seconds).toBe(3600);
	});

	it('saves the last valid duration, once the typing settles', () => {
		toggle(checkbox());
		pick('custom');
		type('120');
		type('2');

		vi.advanceTimersByTime(300);

		expect(ipc.invoke).toHaveBeenCalledWith(
			'prefs_set',
			expect.objectContaining({
				prefs: expect.objectContaining({
					ask_timer: { enabled: true, mode: 'custom', seconds: 120 }
				})
			})
		);
	});

	it('hides the mode and the duration while off, and keeps the chosen values for the next time', () => {
		toggle(checkbox());
		pick('custom');
		type('120');

		toggle(checkbox());
		expect(modes()).toHaveLength(0);
		expect(seconds()).toBeNull();
		expect(app.prefs.ask_timer).toEqual({ enabled: false, mode: 'custom', seconds: 120 });

		toggle(checkbox());
		expect(modes().find((r) => r.value === 'custom')!.checked).toBe(true);
		expect(seconds()!.value).toBe('120');
	});
});

describe('where the section sits', () => {
	it('comes right after the diagram settings in General', () => {
		const general = src('lib/features/settings/GeneralSettings.svelte');
		const diagrams = general.indexOf('<DiagramSettings />');
		const ask = general.indexOf('<AskTimerSettings />');
		expect(diagrams).toBeGreaterThan(0);
		expect(ask).toBeGreaterThan(diagrams);
	});
});
