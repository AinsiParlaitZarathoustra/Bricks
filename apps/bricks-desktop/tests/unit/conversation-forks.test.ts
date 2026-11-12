// @vitest-environment happy-dom
// The forks in the conversation list (ConversationList.svelte) mounted over explicit lists: a fork
// sits under its parent, which has a fold button (aria-expanded) that hides and shows it; a fork
// whose parent is not listed is shown on its own; the actions menu of a row offers « Forker ».
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { flushSync, mount, unmount } from 'svelte';
import ConversationList from '../../src/lib/features/navigation/ConversationList.svelte';
import type { SessionItem } from '../../src/lib/ipc/client';

const ipc = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => {
	class Channel {
		onmessage: ((event: unknown) => void) | null = null;
	}
	return { Channel, invoke: (cmd: string, args?: unknown) => ipc.invoke(cmd, args) };
});
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }));

const PROJECT = 'proj-forks-1';

/** A stored conversation of the project, most recently updated at `updated`. */
function item(id: string, title: string, updated: number, overrides: Partial<SessionItem> = {}): SessionItem {
	return {
		id,
		project_id: PROJECT,
		title,
		created_at: updated,
		updated_at: updated,
		message_count: 2,
		model: 'test/a',
		reasoning: null,
		working_dir: '/work/forks',
		open: false,
		running: false,
		parent_id: null,
		...overrides
	} as SessionItem;
}

let target: HTMLDivElement;
let component: ReturnType<typeof mount> | null = null;

beforeEach(() => {
	ipc.invoke.mockReset();
	ipc.invoke.mockResolvedValue(undefined);
	target = document.createElement('div');
	document.body.appendChild(target);
});

afterEach(() => {
	if (component) void unmount(component);
	component = null;
	target.remove();
	document.body.innerHTML = '';
});

function render(sessions: SessionItem[], limit?: number) {
	component = mount(ConversationList, {
		target,
		props: { sessions, label: 'Conversations du projet', ...(limit === undefined ? {} : { limit }) }
	});
	flushSync();
}

/** The rows of the list, in order, by conversation id. */
const rows = () =>
	Array.from(target.querySelectorAll<HTMLElement>('[data-session-item]')).map((el) => el.dataset.sessionItem);

const row = (id: string) => target.querySelector<HTMLElement>(`[data-session-item="${id}"]`);

/** The fold buttons: one per parent that has forks (the actions menus carry aria-expanded too). */
const folds = () =>
	Array.from(
		target.querySelectorAll<HTMLButtonElement>('button[aria-label^="Masquer les forks"], button[aria-label^="Afficher les forks"]')
	);

/** The fold button of the parent titled `title` (absent when it has no forks). */
const fold = (title: string) => folds().find((b) => (b.getAttribute('aria-label') ?? '').endsWith(` ${title}`));

describe('a fork under its parent', () => {
	it('is listed under its parent, which has a fold button open by default', () => {
		render([
			item('root', 'Plan Atlas', 1_000),
			item('fork', 'Plan Atlas (fork)', 2_000, { parent_id: 'root' }),
			item('other', 'Autre sujet', 1_500)
		]);
		// Shown as given: the fork follows its parent, the other conversation comes after.
		expect(rows()).toEqual(['root', 'fork', 'other']);
		expect(rows().indexOf('fork')).toBe(rows().indexOf('root') + 1);
		const button = fold('Plan Atlas');
		expect(button, 'the parent has a fold button').toBeDefined();
		expect(button!.getAttribute('aria-expanded')).toBe('true');
		expect(button!.getAttribute('aria-label')).toBe('Masquer les forks de Plan Atlas');
	});

	it('is hidden by folding its parent, and shown again by unfolding it', () => {
		render([item('root', 'Plan Atlas', 1_000), item('fork', 'Plan Atlas (fork)', 2_000, { parent_id: 'root' })]);
		expect(row('fork')).not.toBeNull();

		fold('Plan Atlas')!.click();
		flushSync();
		expect(row('fork')).toBeNull();
		expect(row('root')).not.toBeNull();
		const folded = fold('Plan Atlas')!;
		expect(folded.getAttribute('aria-expanded')).toBe('false');
		expect(folded.getAttribute('aria-label')).toBe('Afficher les forks de Plan Atlas');

		fold('Plan Atlas')!.click();
		flushSync();
		expect(row('fork')).not.toBeNull();
		expect(fold('Plan Atlas')!.getAttribute('aria-expanded')).toBe('true');
	});

	it('shows a fork on its own when its parent is not listed', () => {
		render([
			item('orphan', 'Suite orpheline (fork)', 3_000, { parent_id: 'absent' }),
			item('other', 'Autre sujet', 1_000)
		]);
		expect(rows()).toEqual(['orphan', 'other']);
		// Nothing to fold: no fold button on its row, nor on the other one.
		expect(folds().length).toBe(0);
	});

	it('keeps a fork of a fork under the first listed conversation of its line', () => {
		render([
			item('root', 'Plan', 1_000),
			item('middle', 'Plan (fork)', 2_000, { parent_id: 'root', created_at: 2_000 }),
			item('last', 'Plan (fork) (fork)', 3_000, { parent_id: 'middle', created_at: 3_000 })
		]);
		// Only the root has a fold button: both forks are its children, oldest first.
		expect(rows()).toEqual(['root', 'middle', 'last']);
		expect(folds().length).toBe(1);
	});
});

describe('the actions of a conversation', () => {
	it('offer « Forker » in their menu', async () => {
		render([item('root', 'Plan Atlas', 1_000)]);
		const trigger = target.querySelector<HTMLButtonElement>(
			'button[aria-label="Actions pour la conversation Plan Atlas"]'
		);
		expect(trigger, 'the row has an actions button').not.toBeNull();
		trigger!.dispatchEvent(
			new PointerEvent('pointerdown', { bubbles: true, cancelable: true, button: 0, pointerType: 'mouse' })
		);
		flushSync();
		await vi.waitFor(() => {
			const items = Array.from(document.body.querySelectorAll('[role="menuitem"]')).map((el) =>
				el.textContent?.trim()
			);
			expect(items).toContain('Forker');
		});
	});
});

describe('the list limit of a project', () => {
	// The limit counts top-level rows: a fork comes with its line, even when it is recent and
	// its parent is old.
	it('shows a fork whose parent is beyond the limit', () => {
		// The parent is the oldest of four; the fork, just made, is the most recent one.
		render(
			[
				item('fork', 'Plan (fork)', 5_000, { parent_id: 'root' }),
				item('a', 'A', 4_000),
				item('b', 'B', 3_000),
				item('root', 'Plan', 1_000)
			],
			3
		);
		expect(row('fork'), 'the fork is among the three shown').not.toBeNull();
	});
});
