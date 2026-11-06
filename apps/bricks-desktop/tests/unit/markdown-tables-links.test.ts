// @vitest-environment happy-dom
// Tables keep their native layout inside a scrolling wrapper and their Markdown
// alignment; a link keeps the bold or code of its text. Sanitizing is mocked, as in the
// other Markdown tests.
import { readFileSync } from 'node:fs';
import { describe, expect, it, vi } from 'vitest';
import { MarkdownDocument } from '../../src/lib/features/conversation/markdown/document';

vi.mock('../../src/lib/features/conversation/markdown/sanitize', () => ({
	sanitizeMarkdown: (html: string) => html,
	sanitizeSvg: (svg: string) => svg
}));

function html(source: string): string {
	const blocks = new MarkdownDocument({ idPrefix: 't' }).update(source, true);
	return blocks.map((b) => (b.kind === 'html' ? b.html : '')).join('');
}

describe('tables', () => {
	const table = html('| a | b | c |\n|:--|:-:|--:|\n| 1 | 2 | 3 |');

	it('sit in a scrolling wrapper, and stay tables', () => {
		expect(table).toMatch(/<div class="md-table"><table>/);
		expect(table).toMatch(/<\/table>\s*<\/div>/);
	});

	it('keep the alignment of each column', () => {
		expect(table).toContain('<th align="left">a</th>');
		expect(table).toContain('<th align="center">b</th>');
		expect(table).toContain('<td align="center">2</td>');
		expect(table).toContain('<td align="right">3</td>');
	});
});

describe('links', () => {
	it('keep bold and code in their text', () => {
		const link = html('[**bold** and `code`](https://example.com)');
		expect(link).toContain('<strong>bold</strong> and <code>code</code></a>');
	});

	it('stay escaped', () => {
		expect(html('[<b>x</b>](https://example.com)')).not.toContain('<b>');
	});

	it('keep bold in a link to a file', () => {
		expect(html('[**main**](src/main.rs)')).toMatch(/<button[^>]*class="file-ref"[^>]*><strong>main<\/strong><\/button>/);
	});
});

describe('the styles of an answer', () => {
	const view = readFileSync('src/lib/features/conversation/markdown/MarkdownView.svelte', 'utf8');

	it('no longer override the alignment or turn tables into blocks', () => {
		expect(view).not.toContain('text-align: left');
		expect(view).not.toMatch(/:global\(table\) \{\s*display: block/);
	});

	it('keep words whole in table cells', () => {
		expect(view).toMatch(/:global\(td\) \{[^}]*overflow-wrap: normal/);
	});
});
