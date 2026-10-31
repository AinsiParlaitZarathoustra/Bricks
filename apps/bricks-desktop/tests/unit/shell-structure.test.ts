// The window's shell as it is built: the top bar shows the icon of each panel's state, the reader's
// tabs share the width (no tab keeps its width), the composer is a rounded card with no project
// strip, and the start screen shows only its logo, the one that contrasts with the theme last shown.
// The top bar and the start screen are rendered on the server; the others are read from source.
import { existsSync, readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import TopBar from '../../src/lib/layout/TopBar.svelte';
import StartSplash from '../../src/lib/layout/StartSplash.svelte';
import { setTheme } from '../../src/lib/theme/theme.svelte';

const SRC = fileURLToPath(new URL('../../src/', import.meta.url));
const source = (path: string) => readFileSync(join(SRC, path), 'utf8');

/** Every source file under src/, relative to src/. */
function sourceFiles(dir = SRC): string[] {
	return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
		const path = join(dir, entry.name);
		if (entry.isDirectory()) return sourceFiles(path);
		return /\.(svelte|ts|js|css)$/.test(entry.name) ? [path.slice(SRC.length)] : [];
	});
}

/** The visible text of a rendered view: comments and tags left out. */
function visible(html: string): string {
	return html.replace(/<!--[^]*?-->/g, '').replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ').trim();
}

/**
 * The icons of a rendered view, by name. Lucide puts `lucide-<name>` on each icon, with aliases
 * (the left panel is also `lucide-sidebar-open`), and `lucide-icon` on all of them: only names count.
 */
function icons(html: string): string[] {
	return [...html.matchAll(/<svg[^>]*class="([^"]*)"/g)].flatMap((m) =>
		m[1]
			.split(/\s+/)
			.filter((c) => c.startsWith('lucide-') && c !== 'lucide-icon')
			.map((c) => c.slice('lucide-'.length)),
	);
}

describe('TopBar: the icons show the state of each panel', () => {
	const base = {
		onToggleLeft: () => {},
		onToggleRight: () => {},
		onToggleTerminal: () => {},
		onToggleChanges: () => {},
		onToggleReader: () => {},
	};

	it('the left panel shows « open » when hidden and « close » when shown', () => {
		expect(icons(render(TopBar, { props: { ...base, leftPressed: false, rightPressed: false } }).body)).toContain(
			'panel-left-open',
		);
		expect(icons(render(TopBar, { props: { ...base, leftPressed: true, rightPressed: false } }).body)).toContain(
			'panel-left-close',
		);
	});

	it('the right panel shows « open » when hidden and « close » when shown', () => {
		const hidden = icons(render(TopBar, { props: { ...base, leftPressed: false, rightPressed: false } }).body);
		expect(hidden).toContain('panel-right-open');
		expect(hidden).not.toContain('panel-right-close');
		const shown = icons(render(TopBar, { props: { ...base, leftPressed: false, rightPressed: true } }).body);
		expect(shown).toContain('panel-right-close');
	});

	it('the terminal, the changes and the reader have their own icons', () => {
		const names = icons(render(TopBar, { props: { ...base, leftPressed: false, rightPressed: false } }).body);
		expect(names).toContain('terminal');
		expect(names).toContain('git-compare');
		expect(names).toContain('book-open');
	});

	it('the toggles carry their names for assistive technology, and no text is written', () => {
		const html = render(TopBar, { props: { ...base, leftPressed: false, rightPressed: false } }).body;
		expect(html).toContain('aria-label="Panneau de gauche"');
		expect(html).toContain('aria-label="Terminal"');
		expect(visible(html)).toBe('');
	});
});

describe('ReaderPanel: the tabs share the width', () => {
	const panel = source('lib/features/reader/ReaderPanel.svelte');
	const tabLine = panel.split('\n').find((line) => line.includes('max-w-48'));

	it('each tab grows and shrinks between its minimum and its maximum width', () => {
		expect(tabLine).toBeDefined();
		expect(tabLine).toContain('max-w-48 min-w-12 shrink');
	});

	it('no tab keeps its width: the tab does not carry shrink-0', () => {
		expect(tabLine).not.toContain('shrink-0');
	});

	it('the close button of a tab keeps its size', () => {
		const closeLine = panel.split('\n').find((line) => line.includes('size-5'));
		expect(closeLine).toContain('shrink-0');
	});
});

describe('Composer: a rounded card, with no project strip', () => {
	const composer = source('lib/features/composer/Composer.svelte');
	const shell = composer.split('\n').find((line) => line.includes('mt-1 flex flex-col gap-2 rounded-2xl'));

	it('the project strip is gone: its file is deleted and no source imports it', () => {
		expect(existsSync(join(SRC, 'lib/features/composer/ProjectStrip.svelte'))).toBe(false);
		expect(composer).not.toContain('ProjectStrip');
		const importers = sourceFiles().filter((file) => readFileSync(join(SRC, file), 'utf8').includes('ProjectStrip'));
		expect(importers).toEqual([]);
	});

	it('the card is rounded as a rectangle with soft corners, not a pill', () => {
		expect(shell).toBeDefined();
		expect(shell).toContain('rounded-2xl');
		expect(shell).not.toContain('rounded-full');
	});
});

describe('StartSplash: only the logo, the one that contrasts with the theme last shown', () => {
	it('shows no visible text, only the logo', () => {
		setTheme('light');
		const html = render(StartSplash).body;
		expect(visible(html)).toBe('');
		expect(html).toContain('<img');
		expect(html).toContain('alt=""');
	});

	it('is announced as the start of Bricks, without showing it', () => {
		const html = render(StartSplash).body;
		expect(html).toContain('aria-label="Démarrage de Bricks"');
	});

	it('uses the dark tile on a light theme, and the light tile on a dark one', () => {
		setTheme('light');
		expect(render(StartSplash).body).toContain('app-dark.png');
		expect(render(StartSplash).body).not.toContain('app-light.png');
		setTheme('dark');
		expect(render(StartSplash).body).toContain('app-light.png');
		expect(render(StartSplash).body).not.toContain('app-dark.png');
	});
});
