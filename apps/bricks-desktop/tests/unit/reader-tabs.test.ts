// The reader's tabs: one tab per file of one workspace, opened once, told
// apart by their full path, kept per session, closed by the user, and never
// fed by a late answer.
import { describe, expect, it } from 'vitest';
import { ReaderStore } from '../../src/lib/state/reader.svelte';
import { PROJECT, SESSION, WORKTREE, diskRead, manualRead, numbered, settle } from './reader-fixtures';

const target = { sessionId: SESSION, projectId: 'p1' };

describe('opening and activating', () => {
	it('opening the same file again activates its one tab, and counts the request', async () => {
		const { read, requests } = diskRead({ [`${PROJECT}/src/a.rs`]: ['fn a() {}'] });
		const reader = new ReaderStore(read);
		const first = reader.open({ ...target, path: 'src/a.rs' });
		await settle();
		const again = reader.open({ ...target, path: 'src/a.rs' });
		await settle();
		expect(reader.tabs(SESSION)).toHaveLength(1);
		expect(again).toBe(first);
		expect(reader.active(SESSION)).toBe(first);
		expect(reader.openRequests).toBe(2);
		// Without a line, the second open only activates the tab: no second read.
		expect(requests).toHaveLength(1);
	});

	it('a line on an open file reads the window around it again', async () => {
		const { read, requests } = diskRead({ [`${PROJECT}/big.txt`]: numbered(1000) });
		const reader = new ReaderStore(read);
		reader.open({ ...target, path: 'big.txt' });
		await settle();
		reader.open({ ...target, path: 'big.txt', line: 500 });
		await settle();
		expect(reader.tabs(SESSION)).toHaveLength(1);
		expect(requests.at(-1)).toMatchObject({ path: 'big.txt', line: 500, endLine: null });
	});

	it('a relative path first, then the absolute path of the same file: one tab', async () => {
		const { read } = diskRead({ [`${PROJECT}/src/a.rs`]: ['fn a() {}'] });
		const reader = new ReaderStore(read);
		const first = reader.open({ ...target, path: 'src/a.rs' });
		await settle();
		reader.open({ ...target, path: `${PROJECT}/src/a.rs`, absolute: `${PROJECT}/src/a.rs` });
		await settle();
		expect(reader.tabs(SESSION)).toHaveLength(1);
		expect(reader.active(SESSION)).toBe(first);
		expect(first.fullPath).toBe(`${PROJECT}/src/a.rs`);
	});

	it('an absolute path first, then the relative path of the same file: the new tab is merged into the first', async () => {
		const { read } = diskRead({ [`${PROJECT}/src/a.rs`]: ['fn a() {}'] });
		const reader = new ReaderStore(read);
		const first = reader.open({ ...target, path: `${PROJECT}/src/a.rs`, absolute: `${PROJECT}/src/a.rs` });
		await settle();
		reader.open({ ...target, path: 'src/a.rs' });
		await settle();
		expect(reader.tabs(SESSION)).toHaveLength(1);
		expect(reader.active(SESSION)).toBe(first);
	});

	it('the title is the name the conversation gives, never a canonical name', async () => {
		const { read } = diskRead({ [`${PROJECT}/alias.md`]: ['# titre'] });
		const reader = new ReaderStore(read);
		const tab = reader.open({ ...target, path: 'alias.md' });
		await settle();
		expect(tab.title).toBe('alias.md');
		expect(tab.path).toBe('alias.md');
	});

	it('normalises the path asked: a leading ./ is the same file', async () => {
		const { read, requests } = diskRead({ [`${PROJECT}/src/a.rs`]: ['x'] });
		const reader = new ReaderStore(read);
		reader.open({ ...target, path: 'src/a.rs' });
		await settle();
		reader.open({ ...target, path: './src/a.rs' });
		await settle();
		expect(reader.tabs(SESSION)).toHaveLength(1);
		expect(requests).toHaveLength(1);
	});
});

describe('identity of a tab', () => {
	it('two files with the same name are two tabs, told apart by their full path', async () => {
		const { read } = diskRead({
			[`${PROJECT}/src/mod.rs`]: ['// src'],
			[`${PROJECT}/lib/mod.rs`]: ['// lib'],
		});
		const reader = new ReaderStore(read);
		const src = reader.open({ ...target, path: 'src/mod.rs' });
		const lib = reader.open({ ...target, path: 'lib/mod.rs' });
		await settle();
		expect(reader.tabs(SESSION)).toHaveLength(2);
		expect(src.title).toBe('mod.rs');
		expect(lib.title).toBe('mod.rs');
		expect(src.fullPath).toBe(`${PROJECT}/src/mod.rs`);
		expect(lib.fullPath).toBe(`${PROJECT}/lib/mod.rs`);
		expect(src.window?.lines).toEqual(['// src']);
		expect(lib.window?.lines).toEqual(['// lib']);
	});

	it('the same path in the project and in an isolated agent workspace: two tabs, each read in its own workspace', async () => {
		const { read, requests } = diskRead({
			[`${PROJECT}/src/a.rs`]: ['projet'],
			[`${WORKTREE}/src/a.rs`]: ['worktree'],
		});
		const reader = new ReaderStore(read);
		const main = reader.open({ ...target, path: 'src/a.rs' });
		const agent = reader.open({ ...target, path: 'src/a.rs', agentId: 'a1' });
		await settle();
		expect(reader.tabs(SESSION)).toHaveLength(2);
		expect(main.window?.lines).toEqual(['projet']);
		expect(agent.window?.lines).toEqual(['worktree']);
		expect(requests.map((r) => r.agentId)).toEqual([null, 'a1']);
	});

	it('an unknown file gives a readable error on its tab, which can then be closed', async () => {
		const { read } = diskRead({});
		const reader = new ReaderStore(read);
		const tab = reader.open({ ...target, path: 'src/supprime.rs' });
		await settle();
		expect(reader.tabs(SESSION)).toHaveLength(1);
		expect(tab.window).toMatchObject({ loading: false, lines: [] });
		expect(tab.window?.error).toContain("n'existe plus");
		reader.close(tab.id);
		expect(reader.tabs(SESSION)).toHaveLength(0);
	});
});

describe('closing', () => {
	it('closing the last tab leaves no tab and no active tab', async () => {
		const { read } = diskRead({ [`${PROJECT}/a.rs`]: ['a'] });
		const reader = new ReaderStore(read);
		const tab = reader.open({ ...target, path: 'a.rs' });
		await settle();
		reader.close(tab.id);
		expect(reader.tabs(SESSION)).toEqual([]);
		expect(reader.active(SESSION)).toBeNull();
	});

	it('closing the shown tab shows the next one, or the previous one at the end of the list', async () => {
		const { read } = diskRead({ [`${PROJECT}/a.rs`]: ['a'], [`${PROJECT}/b.rs`]: ['b'], [`${PROJECT}/c.rs`]: ['c'] });
		const reader = new ReaderStore(read);
		const a = reader.open({ ...target, path: 'a.rs' });
		const b = reader.open({ ...target, path: 'b.rs' });
		const c = reader.open({ ...target, path: 'c.rs' });
		await settle();
		reader.activate(b.id);
		reader.close(b.id);
		expect(reader.active(SESSION)).toBe(c);
		reader.close(c.id);
		expect(reader.active(SESSION)).toBe(a);
	});

	it('closing a tab that is not shown keeps the shown one', async () => {
		const { read } = diskRead({ [`${PROJECT}/a.rs`]: ['a'], [`${PROJECT}/b.rs`]: ['b'] });
		const reader = new ReaderStore(read);
		const a = reader.open({ ...target, path: 'a.rs' });
		const b = reader.open({ ...target, path: 'b.rs' });
		await settle();
		reader.close(a.id);
		expect(reader.active(SESSION)).toBe(b);
	});
});

describe('state per session', () => {
	it('each session keeps its own tabs and its own shown tab', async () => {
		const { read } = diskRead({ [`${PROJECT}/a.rs`]: ['a'] });
		const reader = new ReaderStore(read);
		const inOne = reader.open({ sessionId: 's1', projectId: 'p1', path: 'a.rs' });
		const inTwo = reader.open({ sessionId: 's2', projectId: 'p1', path: 'a.rs' });
		await settle();
		expect(inOne.id).not.toBe(inTwo.id);
		expect(reader.tabs('s1')).toHaveLength(1);
		expect(reader.tabs('s2')[0]).toBe(inTwo);
		reader.close(inOne.id);
		expect(reader.tabs('s1')).toEqual([]);
		expect(reader.tabs('s2')).toHaveLength(1);
		expect(reader.active('s2')).toBe(inTwo);
		expect(reader.tabs('unknown')).toEqual([]);
		expect(reader.active('unknown')).toBeNull();
	});

	it('forgetting a session drops its tabs without touching the others', async () => {
		const { read } = diskRead({ [`${PROJECT}/a.rs`]: ['a'] });
		const reader = new ReaderStore(read);
		reader.open({ sessionId: 's1', projectId: 'p1', path: 'a.rs' });
		const kept = reader.open({ sessionId: 's2', projectId: 'p1', path: 'a.rs' });
		await settle();
		reader.forget('s1');
		expect(reader.tabs('s1')).toEqual([]);
		expect(reader.active('s1')).toBeNull();
		expect(reader.active('s2')).toBe(kept);
	});
});

describe('windows', () => {
	it('a long file opens on its first 400 lines, flagged as truncated', async () => {
		const { read } = diskRead({ [`${PROJECT}/big.txt`]: numbered(1000) });
		const reader = new ReaderStore(read);
		const tab = reader.open({ ...target, path: 'big.txt' });
		await settle();
		expect(tab.window).toMatchObject({ start: 1, totalLines: 1000, truncated: true, loading: false });
		expect(tab.window?.lines).toHaveLength(400);
	});

	it('loadWindow reads the 400 lines that start at the given line', async () => {
		const { read, requests } = diskRead({ [`${PROJECT}/big.txt`]: numbered(1000) });
		const reader = new ReaderStore(read);
		const tab = reader.open({ ...target, path: 'big.txt' });
		await settle();
		reader.loadWindow(tab.id, 401);
		await settle();
		expect(requests.at(-1)).toMatchObject({ line: 401, endLine: 800 });
		expect(tab.window?.start).toBe(401);
		expect(tab.window?.lines[0]).toBe('ligne 401');
		expect(tab.window?.lines.at(-1)).toBe('ligne 800');
	});

	it('the window while it loads keeps the file totals, and says it is loading', async () => {
		const pending = manualRead();
		const reader = new ReaderStore(pending.read);
		const tab = reader.open({ ...target, path: 'big.txt' });
		expect(tab.window).toMatchObject({ loading: true, lines: [] });
		expect(pending.pending).toHaveLength(1);
	});

	it('an answer to an older request is ignored: the newest window stays', async () => {
		const pending = manualRead();
		const reader = new ReaderStore(pending.read);
		const tab = reader.open({ ...target, path: 'a.rs' });
		reader.loadWindow(tab.id, 2);
		const [older, newer] = pending.pending;
		newer.resolve({ path: 'a.rs', start: 2, lines: ['second'], totalLines: 2, truncated: false, language: null, absolute: `${PROJECT}/a.rs` });
		await settle();
		older.resolve({ path: 'a.rs', start: 1, lines: ['first'], totalLines: 2, truncated: false, language: null, absolute: `${PROJECT}/a.rs` });
		await settle();
		expect(tab.window).toMatchObject({ start: 2, lines: ['second'], loading: false });
	});

	it('an older failure does not replace a newer window', async () => {
		const pending = manualRead();
		const reader = new ReaderStore(pending.read);
		const tab = reader.open({ ...target, path: 'a.rs' });
		reader.loadWindow(tab.id, 2);
		const [older, newer] = pending.pending;
		newer.resolve({ path: 'a.rs', start: 2, lines: ['second'], totalLines: 2, truncated: false, language: null, absolute: `${PROJECT}/a.rs` });
		await settle();
		older.reject(new Error('trop tard'));
		await settle();
		expect(tab.window).toMatchObject({ lines: ['second'], error: null });
	});

	it('an answer that comes after the tab is closed is ignored', async () => {
		const pending = manualRead();
		const reader = new ReaderStore(pending.read);
		const tab = reader.open({ ...target, path: 'a.rs' });
		reader.close(tab.id);
		pending.pending[0].resolve({ path: 'a.rs', start: 1, lines: ['tardif'], totalLines: 1, truncated: false, language: null, absolute: `${PROJECT}/a.rs` });
		await settle();
		expect(tab.window).toMatchObject({ loading: true, lines: [] });
		expect(reader.tabs(SESSION)).toEqual([]);
	});

	it('the scroll position of a tab is kept', async () => {
		const { read } = diskRead({ [`${PROJECT}/a.rs`]: ['a'] });
		const reader = new ReaderStore(read);
		const tab = reader.open({ ...target, path: 'a.rs' });
		reader.rememberScroll(tab.id, 120);
		expect(tab.scrollTop).toBe(120);
		reader.rememberScroll('absent', 5);
		expect(tab.scrollTop).toBe(120);
	});
});
