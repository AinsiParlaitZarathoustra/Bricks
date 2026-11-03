// Guard: French text must not be written in the source of the interface. Every visible text of
// src/lib goes through t() and the catalogues (src/lib/i18n/messages); what stays in the code is
// either a technical value or a proper noun. The scan looks for French-specific characters (accents,
// « », ’) in string literals and in the markup. Comments are not checked. The catalogues and the
// generated IPC bindings are not scanned. ALLOWED lists the few justified exceptions, one by one.
import { readdirSync, readFileSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const LIB = fileURLToPath(new URL('../../src/lib', import.meta.url));
const EXCLUDED = [join(LIB, 'i18n'), join(LIB, 'ipc', 'bindings')];
const FRENCH = /[àâäçéèêëîïôöùûüÿœæÀÂÄÇÉÈÊËÎÏÔÖÙÛÜŸŒÆ’«»]/;

/**
 * Exceptions: a file, a fragment of the line that holds the text, and the reason. The first is a
 * false positive (a regular expression's character class). The others are known defects of
 * src/lib, listed here until they are fixed: each one is a French text the English interface would
 * show as it is, and each must leave this list when it is moved to the catalogues.
 */
const ALLOWED: { file: string; text: string; why: string }[] = [
	{
		file: 'features/conversation/links.ts',
		text: '«»“”',
		why: 'false positive: a character class of a regular expression'
	}
];

interface Finding {
	file: string;
	line: number;
	text: string;
}

function walk(dir: string): string[] {
	return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
		const path = join(dir, entry.name);
		if (EXCLUDED.some((excluded) => path === excluded || path.startsWith(excluded + '/'))) return [];
		if (entry.isDirectory()) return walk(path);
		return /\.(ts|svelte)$/.test(entry.name) && !/\.test\.ts$/.test(entry.name) ? [path] : [];
	});
}

/** The line (1-based) of a character offset. */
function lineOf(text: string, offset: number): number {
	return text.slice(0, offset).split('\n').length;
}

/** The content of each string literal of a script, with its line; comments are skipped. */
function stringsOf(code: string): { line: number; text: string }[] {
	const found: { line: number; text: string }[] = [];
	let i = 0;
	while (i < code.length) {
		const char = code[i];
		const next = code[i + 1];
		if (char === '/' && next === '/') {
			const end = code.indexOf('\n', i);
			i = end < 0 ? code.length : end;
		} else if (char === '/' && next === '*') {
			const end = code.indexOf('*/', i + 2);
			i = end < 0 ? code.length : end + 2;
		} else if (char === "'" || char === '"' || char === '`') {
			const start = i + 1;
			let j = start;
			while (j < code.length && code[j] !== char) j += code[j] === '\\' ? 2 : 1;
			found.push({ line: lineOf(code, start), text: code.slice(start, j) });
			i = j + 1;
		} else {
			i += 1;
		}
	}
	return found;
}

function scan(file: string, source: string): Finding[] {
	const rel = relative(LIB, file);
	if (!file.endsWith('.svelte')) {
		return stringsOf(source)
			.filter((s) => FRENCH.test(s.text))
			.map((s) => ({ file: rel, line: s.line, text: s.text.trim().slice(0, 120) }));
	}
	// A component: its scripts are read as code, its style is skipped, the rest is markup.
	const withoutStyle = source.replace(/<style[\s\S]*?<\/style>/g, (block) => block.replace(/[^\n]/g, ' '));
	const findings: Finding[] = [];
	for (const match of withoutStyle.matchAll(/<script[^>]*>([\s\S]*?)<\/script>/g)) {
		const body = match[1];
		const bodyStart = (match.index ?? 0) + match[0].indexOf('>') + 1;
		for (const s of stringsOf(body)) {
			if (FRENCH.test(s.text)) {
				findings.push({ file: rel, line: lineOf(source, bodyStart) + s.line - 1, text: s.text.trim().slice(0, 120) });
			}
		}
	}
	const markup = withoutStyle
		.replace(/<script[^>]*>[\s\S]*?<\/script>/g, (block) => block.replace(/[^\n]/g, ' '))
		.replace(/<!--[\s\S]*?-->/g, (block) => block.replace(/[^\n]/g, ' '))
		.replace(/\{\/\*[\s\S]*?\*\/\}/g, (block) => block.replace(/[^\n]/g, ' '));
	markup.split('\n').forEach((line, index) => {
		if (FRENCH.test(line)) findings.push({ file: rel, line: index + 1, text: line.trim().slice(0, 120) });
	});
	return findings;
}

describe('no French text is written in the source of src/lib', () => {
	it('finds no hardcoded French outside the catalogues, and no unlisted exception', () => {
		const findings = walk(LIB).flatMap((file) => scan(file, readFileSync(file, 'utf8')));
		const unexpected = findings.filter(
			(f) => !ALLOWED.some((a) => a.file === f.file && f.text.includes(a.text))
		);
		const report = unexpected.map((f) => `${f.file}:${f.line}  ${f.text}`);
		expect(report).toEqual([]);
	});
});
