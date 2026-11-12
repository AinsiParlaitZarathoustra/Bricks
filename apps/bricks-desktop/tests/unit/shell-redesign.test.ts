// The shell redesign: pinned conversations in the app store (order, removal, missing ids, the
// debounced save, a deleted conversation unpinned), the navigation and title markup, the streaming
// caret removed, the contrast notice, the message rail's grains, and the theme cross-fade (only
// when the look changes and reduced motion is not asked for).
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'svelte/server';
import type { SessionItem } from '../../src/lib/ipc/bindings/SessionItem';
import MessageRail from '../../src/lib/features/conversation/MessageRail.svelte';
import { initTheme, setTheme } from '../../src/lib/theme/theme.svelte';
import { DEFAULT_PREFERENCES, app } from '../../src/lib/state/app.svelte';

const ipc = vi.hoisted(() => ({
	prefsSet: vi.fn(),
	sessionDelete: vi.fn(),
	errorMessage: (e: unknown) => (e instanceof Error ? e.message : String(e)),
}));
vi.mock('$lib/ipc/client', () => ipc);
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }));

const source = (path: string) => readFileSync(fileURLToPath(new URL(`../../src/${path}`, import.meta.url)), 'utf8');

const PROJECT = 'proj-shell';

function session(id: string, projectId = PROJECT): SessionItem {
	return {
		id,
		project_id: projectId,
		title: `Conversation ${id}`,
		created_at: 1,
		updated_at: 1,
		message_count: 0,
		model: null,
		reasoning: null,
		working_dir: null,
		open: false,
		running: false,
		parent_id: null,
	};
}

function ids(list: SessionItem[]): string[] {
	return list.map((s) => s.id);
}

beforeEach(() => {
	vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
	ipc.prefsSet.mockReset();
	ipc.prefsSet.mockResolvedValue(undefined);
	ipc.sessionDelete.mockReset();
	ipc.sessionDelete.mockResolvedValue(undefined);
	app.prefs = { ...DEFAULT_PREFERENCES, pinned_sessions: [] };
	app.sessionsByProject.clear();
	app.sessionsByProject.set(PROJECT, [session('s_1'), session('s_2'), session('s_3')]);
});

afterEach(() => {
	vi.clearAllTimers();
	vi.useRealTimers();
	vi.unstubAllGlobals();
});

describe('pinned conversations', () => {
	it('a pin goes to the front, so the last pinned is listed first', () => {
		app.togglePin('s_1');
		app.togglePin('s_2');
		expect(app.prefs.pinned_sessions).toEqual(['s_2', 's_1']);
	});

	it('isPinned tells the pinned from the others', () => {
		app.togglePin('s_2');
		expect(app.isPinned('s_2')).toBe(true);
		expect(app.isPinned('s_3')).toBe(false);
	});

	it('a second toggle removes the pin and leaves the others in order', () => {
		app.togglePin('s_1');
		app.togglePin('s_2');
		app.togglePin('s_1');
		expect(app.isPinned('s_1')).toBe(false);
		expect(app.prefs.pinned_sessions).toEqual(['s_2']);
	});

	it('pinnedSessions keeps the pin order, not the update order', () => {
		app.prefs = { ...app.prefs, pinned_sessions: ['s_3', 's_1'] };
		expect(ids(app.pinnedSessions)).toEqual(['s_3', 's_1']);
	});

	it('pinnedSessions skips the ids that have no conversation', () => {
		app.prefs = { ...app.prefs, pinned_sessions: ['ghost', 's_2', 'gone'] };
		expect(ids(app.pinnedSessions)).toEqual(['s_2']);
	});

	it('the pins are saved once, after a short pause', () => {
		app.togglePin('s_1');
		expect(ipc.prefsSet).not.toHaveBeenCalled();
		vi.advanceTimersByTime(300);
		expect(ipc.prefsSet).toHaveBeenCalledTimes(1);
		expect(ipc.prefsSet.mock.calls[0][0].pinned_sessions).toEqual(['s_1']);
	});

	it('deleting a pinned conversation unpins it', async () => {
		app.togglePin('s_1');
		app.togglePin('s_2');
		expect(await app.deleteSession('s_1')).toBe(true);
		expect(app.isPinned('s_1')).toBe(false);
		expect(app.prefs.pinned_sessions).toEqual(['s_2']);
		vi.advanceTimersByTime(300);
		expect(ipc.prefsSet.mock.calls.at(-1)?.[0].pinned_sessions).toEqual(['s_2']);
	});
});

describe('navigation and title markup', () => {
	it('the new-project button is an icon labelled by its name, beside the Projects heading', () => {
		const nav = source('lib/features/navigation/NavigationPanel.svelte');
		expect(nav).toContain("aria-label={t('navigation.newProject')}");
		expect(nav).not.toMatch(/>\s*\{t\('navigation\.newProject'\)\}/);
		expect(nav.indexOf("t('navigation.projects')")).toBeLessThan(nav.indexOf('<FolderPlus'));
	});

	it('the pinned section comes before the projects', () => {
		const nav = source('lib/features/navigation/NavigationPanel.svelte');
		expect(nav.indexOf('app.pinnedSessions.length > 0')).toBeGreaterThan(-1);
		expect(nav.indexOf('app.pinnedSessions.length > 0')).toBeLessThan(nav.indexOf("t('navigation.projects')"));
	});

	it('the settings footer has no top border', () => {
		const nav = source('lib/features/navigation/NavigationPanel.svelte');
		const footer = nav.slice(nav.indexOf('</ScrollArea>'));
		expect(footer).toContain('ui.settingsOpen = true');
		expect(footer).not.toContain('border-t');
	});

	it('the project name in the title opens a new conversation in that project', () => {
		expect(source('lib/features/conversation/ConversationTitle.svelte')).toContain('app.newConversation(project.id)');
	});
});

describe('the streaming caret is gone', () => {
	it('the markdown view has no caret rule or keyframes', () => {
		const md = source('lib/features/conversation/markdown/MarkdownView.svelte');
		expect(md).not.toContain('::after');
		expect(md).not.toContain('@keyframes caret');
	});

	it('the assistant message has no pulsing bar', () => {
		expect(source('lib/features/conversation/timeline/AssistantMessage.svelte')).not.toContain('animate-pulse');
	});
});

describe('colour choices', () => {
	it('a colour row shows its name only, with no theme-colour caption', () => {
		expect(source('lib/features/settings/ColorChoice.svelte')).not.toContain('settings.themeColor');
	});
});

describe('the message rail', () => {
	const marks = (count: number) =>
		Array.from({ length: count }, (_, i) => ({ key: `turn-${i}`, label: `Message ${i + 1}`, answer: '', at: null, place: i + 1, total: count }));

	it('renders nothing for fewer than two messages', () => {
		for (const count of [0, 1]) {
			const html = render(MessageRail, { props: { marks: marks(count), scroller: null } }).body;
			expect(html).not.toContain('<nav');
			expect(html).not.toContain('<button');
		}
	});

	it('renders one button per message, named by the message start', () => {
		const html = render(MessageRail, { props: { marks: marks(3), scroller: null } }).body;
		expect(html).toContain('<nav');
		expect(html.match(/<button/g)).toHaveLength(3);
		expect(html).toContain('aria-label="Message 1 · 1 sur');
		expect(html).toContain('aria-label="Message 3 · 3 sur 3"');
	});
});

describe('the theme cross-fade', () => {
	/** Stubs the parts of document and window the theme reads; a transition runs its update at once. */
	function stubDom(options: { reducedMotion?: boolean; viewTransitions?: boolean }) {
		const classes = new Set<string>();
		const startViewTransition = vi.fn((update: () => void) => {
			update();
			return {
				finished: Promise.resolve(),
				ready: Promise.resolve(),
				updateCallbackDone: Promise.resolve(),
				skipTransition: () => {},
			};
		});
		vi.stubGlobal('document', {
			hidden: false,
			documentElement: {
				classList: {
					toggle: (name: string, force?: boolean) => {
						if (force) classes.add(name);
						else classes.delete(name);
						return Boolean(force);
					},
				},
			},
			...(options.viewTransitions === false ? {} : { startViewTransition }),
		});
		vi.stubGlobal('window', {
			matchMedia: (query: string) => ({
				matches: query.includes('reduce') ? options.reducedMotion === true : false,
				addEventListener: () => {},
				removeEventListener: () => {},
			}),
		});
		return { classes, startViewTransition };
	}

	it('fades a change of look', () => {
		const { classes, startViewTransition } = stubDom({});
		const dispose = initTheme('light');
		setTheme('dark');
		expect(startViewTransition).toHaveBeenCalledTimes(1);
		expect(classes.has('dark')).toBe(true);
		dispose();
	});

	it('does not fade when reduced motion is asked for, but still changes the look', () => {
		const { classes, startViewTransition } = stubDom({ reducedMotion: true });
		const dispose = initTheme('light');
		setTheme('dark');
		expect(startViewTransition).not.toHaveBeenCalled();
		expect(classes.has('dark')).toBe(true);
		dispose();
	});

	it('does not fade when the look shown stays the same', () => {
		const { classes, startViewTransition } = stubDom({});
		const dispose = initTheme('system');
		setTheme('light');
		expect(startViewTransition).not.toHaveBeenCalled();
		expect(classes.has('dark')).toBe(false);
		dispose();
	});

	it('changes the look at once where the API is missing', () => {
		const { classes } = stubDom({ viewTransitions: false });
		const dispose = initTheme('light');
		setTheme('dark');
		expect(classes.has('dark')).toBe(true);
		dispose();
	});
});
