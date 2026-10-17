// User messages: web addresses become link parts, code and HTML-like text do not.
// Pure functions of src/lib/features/conversation/links.ts (rendered by markup, never by {@html}).
import { describe, expect, it } from 'vitest';
import { hrefFor, isOpenableUrl, messageParts, trimAddress, type MessagePart } from '../../src/lib/features/conversation/links';

const links = (parts: MessagePart[]) => parts.filter((p) => p.kind === 'link');
const textOf = (parts: MessagePart[]) => parts.map((p) => p.text).join('');

describe('messageParts: web addresses in plain text', () => {
	it('turns an https address into a link and keeps the surrounding text', () => {
		expect(messageParts('Voir https://example.com/page pour plus')).toEqual([
			{ kind: 'text', text: 'Voir ' },
			{ kind: 'link', text: 'https://example.com/page', href: 'https://example.com/page' },
			{ kind: 'text', text: ' pour plus' }
		]);
	});

	it('links a www. address with https as the target', () => {
		const parts = messageParts('Docs : www.example.com/guide.');
		expect(links(parts)).toEqual([{ kind: 'link', text: 'www.example.com/guide', href: 'https://www.example.com/guide' }]);
		expect(textOf(parts)).toBe('Docs : www.example.com/guide.');
	});

	it('leaves the final punctuation out of the link', () => {
		expect(links(messageParts('Fin https://example.com/end.'))[0]).toMatchObject({ text: 'https://example.com/end' });
		expect(links(messageParts('Question https://example.com/q?'))[0]).toMatchObject({ text: 'https://example.com/q' });
		expect(links(messageParts('Liste www.a.com, www.b.com;'))).toMatchObject([
			{ text: 'www.a.com', href: 'https://www.a.com' },
			{ text: 'www.b.com', href: 'https://www.b.com' }
		]);
	});

	it('keeps a closing bracket that the address opens, and drops one it does not', () => {
		expect(links(messageParts('voir (https://example.com/p_(1))'))[0]).toMatchObject({ text: 'https://example.com/p_(1)' });
		expect(links(messageParts('entre parenthèses (https://example.com/x)'))[0]).toMatchObject({ text: 'https://example.com/x' });
	});

	it('keeps an ampersand and a query string whole', () => {
		const parts = messageParts('Lien https://example.com/s?a=1&b=2&c=3 ici');
		expect(links(parts)).toEqual([
			{ kind: 'link', text: 'https://example.com/s?a=1&b=2&c=3', href: 'https://example.com/s?a=1&b=2&c=3' }
		]);
	});

	it('stops at quotes and angle brackets', () => {
		expect(links(messageParts('« https://example.com/a » et "https://example.com/b"'))).toMatchObject([
			{ text: 'https://example.com/a' },
			{ text: 'https://example.com/b' }
		]);
	});

	it('does not link a word that merely contains www. or a scheme', () => {
		expect(links(messageParts('xwww.example.com et ahttps://nope.example'))).toEqual([]);
	});

	it('keeps text unchanged when there is no address', () => {
		expect(messageParts('bonjour, ça va ?')).toEqual([{ kind: 'text', text: 'bonjour, ça va ?' }]);
		expect(messageParts('')).toEqual([]);
	});

	it('keeps every character of the message, in order', () => {
		const text = 'a https://x.example/1. b www.y.example, c &amp; <b>d</b>';
		expect(textOf(messageParts(text))).toBe(text);
	});
});

describe('messageParts: code stays text', () => {
	it('does not link an address in an inline code span', () => {
		expect(links(messageParts('lance `curl https://api.example/v1` puis'))).toEqual([]);
	});

	it('does not link an address in a fenced block', () => {
		const text = 'exemple :\n```\nhttps://fenced.example\n```\nfin';
		expect(links(messageParts(text))).toEqual([]);
		expect(textOf(messageParts(text))).toBe(text);
	});

	it('still links an address outside the code', () => {
		const parts = messageParts('`code` puis https://outside.example');
		expect(links(parts)).toMatchObject([{ text: 'https://outside.example' }]);
	});
});

describe('messageParts: HTML-like text is text', () => {
	it('keeps markup as text and only the address becomes a link', () => {
		const text = '<img src=x onerror="alert(1)"> https://x.example';
		const parts = messageParts(text);
		expect(links(parts)).toMatchObject([{ text: 'https://x.example' }]);
		expect(textOf(parts)).toBe(text);
	});
});

describe('trimAddress and hrefFor', () => {
	it('trims sentence punctuation only at the end', () => {
		expect(trimAddress('https://a.example/x.')).toBe('https://a.example/x');
		expect(trimAddress('https://a.example/x.y')).toBe('https://a.example/x.y');
		expect(trimAddress('https://a.example/x...')).toBe('https://a.example/x');
	});

	it('accepts only web addresses with a host', () => {
		expect(hrefFor('https://')).toBeNull();
		expect(hrefFor('http:///path')).toBeNull();
		expect(hrefFor('javascript:alert(1)')).toBeNull();
		expect(hrefFor('www.example.com')).toBe('https://www.example.com');
		expect(hrefFor('http://localhost:1420/x')).toBe('http://localhost:1420/x');
	});
});

describe('isOpenableUrl: the schemes that may be opened', () => {
	it('allows http, https and mailto only', () => {
		expect(isOpenableUrl('https://example.com')).toBe(true);
		expect(isOpenableUrl('http://example.com')).toBe(true);
		expect(isOpenableUrl('mailto:a@example.com')).toBe(true);
		expect(isOpenableUrl('javascript:alert(1)')).toBe(false);
		expect(isOpenableUrl('file:///etc/passwd')).toBe(false);
		expect(isOpenableUrl('not a url')).toBe(false);
	});
});
