// Static checks of the.3 interface (validation, Haiku V): the texts that spec §6 removes
// are gone from the views that render them, the wording and structure the spec keeps are there,
// and no raw HTML is rendered outside the sanitised Markdown path. These read the source files;
// they do not render components. A `it.fails` documents a known open defect: it passes while the
// defect is present and must be turned into `it` once the product is fixed.
import { readFileSync, readdirSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

const src = (path: string) => readFileSync(new URL(`../../src/${path}`, import.meta.url), 'utf8');

const forbidden = (text: string, phrases: readonly string[]) => phrases.filter((p) => text.includes(p));

/** Texts of spec §6 for « Modifications → Historique ». */
const HISTORY_LEFTOVERS = [
	'Aucune modification enregistrée pour ce projet.',
	'Enregistrées',
	'Non attribuées',
	'Chiffres :',
	'somme des opérations',
	'Aucune modification',
	'Les modifications faites par les agents'
];

describe('Modifications → Historique has no leftover text (spec §6)', () => {
	it('the history tab renders none of the removed sentences, nor the edit-sum label', () => {
		const text = src('lib/features/changes/HistoryChanges.svelte');
		expect(forbidden(text, HISTORY_LEFTOVERS)).toEqual([]);
		expect(text).not.toContain('EDIT_SUM_LABEL');
	});

	it('an empty journal renders nothing under the tab (no title, no description)', () => {
		const text = src('lib/features/changes/HistoryChanges.svelte');
		expect(text).toContain("shown.view === 'entries'");
		expect(text).not.toMatch(/<EmptyState[^>]*title="Aucune/);
	});

		// Same family as spec §6: the Conversation tab of the same panel must not keep the removed
	// empty-state title or the permanent edit-sum sentence (the countLabel prop is gone with them).
	it('the Conversation tab does not keep the removed empty title or the permanent edit-sum sentence', () => {
		const text = src('lib/features/changes/ConversationChanges.svelte');
		expect(forbidden(text, ['title="Aucune modification"', 'Chiffres :'])).toEqual([]);
	});
});

describe('the run-end card and the composer keep only what the spec keeps', () => {
	it('RunEndCard has no « Durée : » prefix, no turn count and no model identifier in its markup', () => {
		const text = src('lib/features/conversation/timeline/RunEndCard.svelte');
		expect(text).not.toContain('Durée');
		expect(text).not.toMatch(/\bturns\b/);
		expect(text).not.toMatch(/\brun\.model\b|\bmodel\}/);
	});

	it('the composer no longer says a draft is kept while a run goes on', () => {
		const composer = src('lib/features/composer/Composer.svelte');
		expect(forbidden(composer, ['Une génération est en cours', 'le brouillon sera conservé'])).toEqual([]);
	});

	it('the send button tooltip is « Envoyer » and the placeholder « Quelque chose à dire ? » stays', () => {
		const composer = src('lib/features/composer/Composer.svelte');
		expect(composer).toContain('<Tooltip.Content side="top">Envoyer</Tooltip.Content>');
		expect(composer).toContain("'Quelque chose à dire ?'");
	});
});

describe('model form: two limits, no context window field', () => {
	it('the form says « Nom API » and keeps the two token limits, and no longer edits the window', () => {
		const text = src('lib/features/settings/ModelFormDialog.svelte');
		expect(text).toContain('Nom API');
		expect(text).toContain('Entrée maximale (tokens)');
		expect(text).toContain('Sortie maximale (tokens)');
		expect(forbidden(text, ['Modèle de l’API', "Modèle de l'API", 'Fenêtre de contexte', 'context_window'])).toEqual([]);
	});

	it('the model card shows the two limits and no context window', () => {
		const text = src('lib/features/settings/ModelRow.svelte');
		expect(text).toContain('Entrée maximale');
		expect(text).toContain('Sortie maximale');
		expect(forbidden(text, ['Fenêtre de contexte', 'context_window'])).toEqual([]);
	});
});

describe('appearance: bundled Google Sans fonts', () => {
	it('uses the Google families without old font declarations or network imports', () => {
		const css = ['app.css', 'styles/fonts.css', 'styles/tokens.generated.css']
			.map((path) => src(path)).join('\n');
		const main = src('main.ts');
		expect(css).not.toMatch(/font-family:\s*['"]?Inter\b/);
		expect(css).not.toMatch(/--font-sans:[^;]*Inter/);
		expect(main).not.toContain('fontsource');
		expect(css).not.toMatch(/Geist/i);
		expect(css).toContain("--ui-font-family: 'Google Sans Flex'");
		expect(css).toContain("--code-font-family: 'Google Sans Code'");
	});
});

describe('header: title and project centred, controls outside the centre', () => {
	it('TopBar uses three columns whose sides are equal, so the centre does not move with the buttons', () => {
		const text = src('lib/layout/TopBar.svelte');
		expect(text).toContain('grid-cols-[minmax(0,1fr)_minmax(0,min(28rem,50%))_minmax(0,1fr)]');
		expect(text).toContain('data-tauri-drag-region');
	});

	it('the left toggle sits in the first column, the title in the centre, the actions in the last', () => {
		const text = src('lib/layout/TopBar.svelte');
		const left = text.indexOf('justify-self-start');
		const centre = text.indexOf('justify-center');
		const right = text.indexOf('justify-self-end');
		expect(left).toBeGreaterThan(0);
		expect(centre).toBeGreaterThan(left);
		expect(right).toBeGreaterThan(centre);
	});
});

describe('text direction and links', () => {
	it('the composer textarea and the user bubble are automatic-direction and logical-aligned', () => {
		for (const path of ['lib/features/composer/Composer.svelte', 'lib/features/conversation/timeline/UserMessage.svelte']) {
			const text = src(path);
			expect(text, path).toContain('dir="auto"');
			expect(text, path).toContain('text-start');
		}
	});

	it('raw HTML is rendered only by the sanitised Markdown path', () => {
		const files: string[] = [];
		const walk = (dir: string) => {
			for (const entry of readdirSync(new URL(`../../src/${dir}`, import.meta.url), { withFileTypes: true })) {
				const rel = `${dir}/${entry.name}`;
				if (entry.isDirectory()) walk(rel);
				// Local « … 2 » copies of the user are not sources.
				else if (entry.name.endsWith('.svelte') && !/ \d+\.svelte$/.test(entry.name)) files.push(rel);
			}
		};
		walk('lib');
		const withHtml = files
			.filter((f) => readFileSync(new URL(`../../src/${f}`, import.meta.url), 'utf8').includes('{@html'))
			.sort();
		// Message blocks come from the Markdown document (sanitizeMarkdown/sanitizeKatex);
		// diagrams use a sanitized SVG inside an opaque, scriptless iframe.
		expect(withHtml).toEqual([
			'lib/features/conversation/markdown/MarkdownView.svelte',
			'lib/features/reader/ReaderPanel.svelte',
		]);
		expect(src('lib/features/conversation/markdown/MarkdownView.svelte')).toContain('MarkdownDocument');
		expect(src('lib/features/conversation/timeline/AssistantMessage.svelte')).toContain('MarkdownView');
		expect(src('lib/features/conversation/markdown/sanitize.ts')).toContain('DOMPurify.sanitize');
		expect(src('lib/features/conversation/markdown/svg-viewer.ts')).toContain('sanitizeSvg');
		expect(src('lib/features/conversation/markdown/SvgViewer.svelte')).toContain('sandbox=""');
		expect(src('lib/features/conversation/markdown/svg-viewer.ts')).toContain("script-src 'none'");
	});

	it('user messages never go through Markdown or HTML: they are built from text parts', () => {
		const text = src('lib/features/conversation/timeline/UserMessage.svelte');
		expect(text).not.toContain('renderMarkdown');
		expect(text).toContain('messageParts');
	});
});
