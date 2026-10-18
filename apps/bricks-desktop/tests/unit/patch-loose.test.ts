// Reading of change proposals (`loose`) and of multi-file texts (`patchFiles`); the
// default reading, used by the Git views, must not change.
import { describe, expect, it } from 'vitest';
import { countPatch, parsePatch, patchFiles } from '../../src/lib/features/changes/patch';

const kinds = (text: string, loose = false) =>
	parsePatch(text, { loose }).map((line) => line.kind);

describe('parsePatch default reading (Git views)', () => {
	it('counts only the lines inside hunks', () => {
		const git = [
			'diff --git a/a.ts b/a.ts',
			'--- a/a.ts',
			'+++ b/a.ts',
			'@@ -1,2 +1,2 @@',
			'-old',
			'+new',
			' same',
			''
		].join('\n');
		expect(countPatch(git)).toEqual({ added: 1, removed: 1 });
	});

	it('does not count bare +/- lines outside a hunk', () => {
		const bare = '+added\n-removed\n';
		expect(kinds(bare)).toEqual(['meta', 'meta']);
		expect(countPatch(bare)).toEqual({ added: 0, removed: 0 });
	});
});

describe('parsePatch loose reading', () => {
	it('numbers the new side from its start, not the old count', () => {
		const lines = parsePatch('@@ -10,2 +40,2 @@\n same\n-old\n+new\n', { loose: true });
		expect(lines.slice(1).map(({ oldNo, newNo }) => [oldNo, newNo])).toEqual([
			[10, 40], [11, null], [null, 41]
		]);
	});

	it('numbers a hunk with an omitted old count', () => {
		const lines = parsePatch('@@ -10 +40,2 @@\n-old\n+new\n+more\n', { loose: true });
		expect(lines.slice(1).map(({ oldNo, newNo }) => [oldNo, newNo])).toEqual([[10, null], [null, 40], [null, 41]]);
	});

	it('numbers a hunk with omitted counts on both sides', () => {
		const lines = parsePatch('@@ -7 +19 @@\n-old\n+new\n', { loose: true });
		expect(lines.slice(1).map(({ oldNo, newNo }) => [oldNo, newNo])).toEqual([[7, null], [null, 19]]);
	});

	it('counts the bare +/- lines of a proposal without any hunk header', () => {
		const proposal = '--- a/x.ts\n+++ b/x.ts\n+const a = 1\n-const b = 2\n';
		expect(kinds(proposal, true)).toEqual(['meta', 'meta', 'add', 'del']);
		const counted = parsePatch(proposal, { loose: true });
		expect(counted[2]).toMatchObject({ kind: 'add', text: '+const a = 1', oldNo: null, newNo: null });
	});

	it('ignores bare +/- lines when the text has hunks', () => {
		const mixed = '+stray\n@@ -1,1 +1,1 @@\n-a\n+b\n';
		expect(kinds(mixed, true)).toEqual(['meta', 'hunk', 'del', 'add']);
		expect(countPatch(mixed)).toEqual({ added: 1, removed: 1 });
	});

	it('takes a removed line that starts with "-- " inside a hunk for a line, not a file header', () => {
		const tricky = '@@ -1,2 +1,2 @@\n--- old\n+++ new\n ctx\n';
		expect(kinds(tricky, true)).toEqual(['hunk', 'del', 'add', 'context']);
	});

	it('restarts the counts at each file header pair', () => {
		const two = '--- a/x\n+++ b/x\n+one\n--- a/y\n+++ b/y\n+two\n-three\n';
		expect(countPatch(two)).toEqual({ added: 0, removed: 0 });
		const counted = parsePatch(two, { loose: true }).filter((l) => l.kind === 'add' || l.kind === 'del');
		expect(counted.map((l) => l.kind)).toEqual(['add', 'add', 'del']);
	});
});

describe('patchFiles', () => {
	it('splits a Git diff by file, with its own paths and counts', () => {
		const git = [
			'diff --git a/src/one.ts b/src/one.ts',
			'index 1111111..2222222 100644',
			'--- a/src/one.ts',
			'+++ b/src/one.ts',
			'@@ -1 +1,2 @@',
			'-a',
			'+b',
			'+c',
			'diff --git a/gone.md b/gone.md',
			'deleted file mode 100644',
			'--- a/gone.md',
			'+++ /dev/null',
			'@@ -1,2 +0,0 @@',
			'-x',
			'-y',
			''
		].join('\n');
		const files = patchFiles(git);
		expect(files.map((f) => [f.path, f.additions, f.deletions])).toEqual([
			['src/one.ts', 2, 1],
			['gone.md', 0, 2]
		]);
		expect(files[0].patch.startsWith('diff --git a/src/one.ts')).toBe(true);
		expect(files[0].patch.endsWith('\n')).toBe(true);
		expect(files.every((f) => !f.binary)).toBe(true);
	});

	it('splits a proposal without headers into files by file-header pairs', () => {
		const proposal = '--- a/x.ts\n+++ b/x.ts\n+one\n--- a/y.ts\n+++ b/y.ts\n+two\n-three\n';
		const files = patchFiles(proposal);
		expect(files.map((f) => [f.path, f.additions, f.deletions])).toEqual([
			['x.ts', 1, 0],
			['y.ts', 1, 1]
		]);
	});

	it('gives a null path when the text names none, and counts its bare lines', () => {
		expect(patchFiles('+a\n-b\n')).toEqual([
			{ path: null, patch: '+a\n-b\n', additions: 1, deletions: 1, binary: false }
		]);
	});

	it('flags binary sections', () => {
		const git = [
			'diff --git a/img.png b/img.png',
			'index 1..2 100644',
			'Binary files a/img.png and b/img.png differ',
			''
		].join('\n');
		const [file] = patchFiles(git);
		expect(file.binary).toBe(true);
		expect(file.path).toBe('img.png');
		expect(file.additions + file.deletions).toBe(0);
	});

	it('unquotes the paths Git quotes (octal bytes, escapes)', () => {
		const git = String.raw`diff --git "a/caf\303\251 \"x\".md" "b/caf\303\251 \"x\".md"
--- "a/caf\303\251 \"x\".md"
+++ "b/caf\303\251 \"x\".md"
@@ -1 +1 @@
-a
+b
`;
		const [file] = patchFiles(git);
		expect(file.path).toBe('café "x".md');
		expect(file.additions).toBe(1);
	});

	it('takes the name after the tab of a timestamped header', () => {
		const proposal = '--- a/t.txt\t2026-01-01 10:00:00\n+++ b/t.txt\t2026-01-01 10:01:00\n+ligne\n';
		expect(patchFiles(proposal)[0].path).toBe('t.txt');
	});

	it('returns nothing for an empty text', () => {
		expect(patchFiles('')).toEqual([]);
		expect(patchFiles('\n\n')).toEqual([]);
	});
});
