import { mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { build, createLogger, createServer } from 'vite';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { designTokensPlugin } from '../../scripts/design-tokens-plugin.js';

const servers: Awaited<ReturnType<typeof createServer>>[] = [];
const tempDirs: string[] = [];

async function waitForWatchedFile(server: Awaited<ReturnType<typeof createServer>>, file: string): Promise<void> {
	const absoluteFile = path.resolve(file);
	const basename = path.basename(absoluteFile);
	await vi.waitFor(() => {
		const watched = server.watcher.getWatched();
		const registered = Object.entries(watched).some(
			([directory, names]) => path.resolve(directory, basename) === absoluteFile && names.includes(basename),
		);
		if (!registered) throw new Error(`Vite watcher has not registered ${absoluteFile}`);
	}, { timeout: 4_000 });
}

// A change written right after the watcher starts can be missed (macOS
// FSEvents, Linux inotify): `rewrite` writes the source again every second
// until `done` holds.
async function rewriteUntil(done: () => boolean, rewrite: () => void, what: string): Promise<void> {
	rewrite();
	const deadline = Date.now() + 10_000;
	let next = Date.now() + 1_000;
	while (Date.now() < deadline) {
		if (done()) return;
		if (Date.now() >= next) {
			rewrite();
			next = Date.now() + 1_000;
		}
		await new Promise((resolve) => setTimeout(resolve, 25));
	}
	throw new Error(`Timed out waiting for ${what}`);
}

function contains(file: string, expected: string): boolean {
	try {
		return readFileSync(file, 'utf8') === expected;
	} catch {
		// The file may not exist until the first valid source is generated.
		return false;
	}
}

async function waitForFile(file: string, expected: string, rewrite: () => void): Promise<void> {
	await rewriteUntil(() => contains(file, expected), rewrite, `${file} to contain ${expected}`);
}

afterEach(async () => {
	await Promise.all(servers.splice(0).map((server) => server.close()));
	for (const directory of tempDirs.splice(0)) rmSync(directory, { recursive: true, force: true });
});

describe('design token Vite plugin', () => {
	it('keeps builds strict when token generation fails', async () => {
		const root = mkdtempSync(path.join(os.tmpdir(), 'bricks-token-build-'));
		tempDirs.push(root);
		const sourceFile = path.join(root, 'tokens.json');
		writeFileSync(sourceFile, '{ incomplete json');
		const generate = (): string[] => {
			JSON.parse(readFileSync(sourceFile, 'utf8'));
			return [];
		};

		await expect(
			build({
				configFile: false,
				root,
				logLevel: 'silent',
				plugins: [designTokensPlugin(sourceFile, generate)],
			}),
		).rejects.toThrow(/SyntaxError: Expected property name/);
	});

	it('keeps dev running on invalid startup JSON and regenerates after it is fixed', async () => {
		const root = mkdtempSync(path.join(os.tmpdir(), 'bricks-token-vite-'));
		tempDirs.push(root);
		const sourceFile = path.join(root, 'tokens.json');
		const cssFile = path.join(root, 'tokens.css');
		writeFileSync(sourceFile, '{ incomplete json');
		writeFileSync(cssFile, 'last-good-css');

		const logger = createLogger('silent');
		const errorSpy = vi.spyOn(logger, 'error');
		const generate = (): string[] => {
			const tokens = JSON.parse(readFileSync(sourceFile, 'utf8')) as { css: string };
			writeFileSync(cssFile, tokens.css);
			return [cssFile];
		};
		const server = await createServer({
			configFile: false,
			root,
			customLogger: logger,
			server: { host: '127.0.0.1', port: 0 },
			plugins: [designTokensPlugin(sourceFile, generate)],
		});
		servers.push(server);
		await server.listen();
		await waitForWatchedFile(server, sourceFile);

		expect(server.httpServer?.listening).toBe(true);
		expect(readFileSync(cssFile, 'utf8')).toBe('last-good-css');
		expect(errorSpy).toHaveBeenCalledWith(
			expect.stringContaining('[bricks-design-tokens] Could not generate theme CSS:'),
			expect.objectContaining({ error: expect.any(SyntaxError) }),
		);

		await waitForFile(cssFile, 'recovered-css', () => writeFileSync(sourceFile, JSON.stringify({ css: 'recovered-css' })));

		await rewriteUntil(
			() => errorSpy.mock.calls.length >= 2,
			() => writeFileSync(sourceFile, '{ temporarily incomplete'),
			'the error on the incomplete source',
		);
		expect(readFileSync(cssFile, 'utf8')).toBe('recovered-css');

		await waitForFile(cssFile, 'recovered-again', () => writeFileSync(sourceFile, JSON.stringify({ css: 'recovered-again' })));
	}, 30_000);
});
