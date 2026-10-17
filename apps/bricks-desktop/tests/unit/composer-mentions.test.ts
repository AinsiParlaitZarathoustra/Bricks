// `@` mentions: detection of the token at the caret, its removal once attached,
// the absolute path of a project entry and the attachment kind.
import { describe, expect, it } from 'vitest';
import {
	mentionAt,
	mentionKind,
	projectEntryPath,
	removeMention
} from '../../src/lib/features/composer/mentions';

describe('mentionAt: the token at the caret', () => {
	it('is the bare @ at the start of the text', () => {
		expect(mentionAt('@', 1)).toEqual({ start: 0, end: 1, query: '' });
	});

	it('takes the text typed after the @, paths included', () => {
		expect(mentionAt('bonjour @src/ma', 15)).toEqual({ start: 8, end: 15, query: 'src/ma' });
	});

	it('starts a token after a space or a line break', () => {
		expect(mentionAt('lis @README', 11)?.query).toBe('README');
		expect(mentionAt('ligne\n@fi', 9)?.query).toBe('fi');
	});

	it('uses the last @ of the text before the caret', () => {
		expect(mentionAt('see @a and @b', 13)).toEqual({ start: 11, end: 13, query: 'b' });
	});

	it('accepts the caret right before a space that follows the token', () => {
		// Caret after `@b`, a space and `c` follow: the token still ends at the caret.
		expect(mentionAt('a @b c', 4)).toEqual({ start: 2, end: 4, query: 'b' });
	});

	it('is null in an e-mail address or after a letter', () => {
		expect(mentionAt('mail@example.com', 16)).toBeNull();
		expect(mentionAt('a@', 2)).toBeNull();
	});

	it('is null once a space has been typed after the query', () => {
		expect(mentionAt('@foo ', 5)).toBeNull();
	});

	it('is null when text still follows the caret inside the token', () => {
		// Caret between `@a` and `bc`: the token is not finished where the caret is.
		expect(mentionAt('x @abc', 4)).toBeNull();
	});

	it('is null without an @, on an empty text, or with a caret out of range', () => {
		expect(mentionAt('bonjour', 7)).toBeNull();
		expect(mentionAt('', 0)).toBeNull();
		expect(mentionAt('@a', 5)).toBeNull();
		expect(mentionAt('@a', -1)).toBeNull();
	});
});

describe('removeMention: the token leaves the text', () => {
	it('removes the token and the space before it', () => {
		const text = 'hello @foo';
		expect(removeMention(text, { start: 6, end: 10, query: 'foo' })).toEqual({ text: 'hello ', caret: 6 });
	});

	it('removes one space that separated the token from the text after it', () => {
		const text = 'hello @foo world';
		expect(removeMention(text, { start: 6, end: 10, query: 'foo' })).toEqual({ text: 'hello world', caret: 6 });
	});

	it('removes a token at the start of the text with its following space', () => {
		expect(removeMention('@foo bar', { start: 0, end: 4, query: 'foo' })).toEqual({ text: 'bar', caret: 0 });
	});
});

describe('projectEntryPath: absolute path of an entry', () => {
	it('joins the project root and the relative path', () => {
		expect(projectEntryPath('/Users/me/proj', 'src/app.ts')).toBe('/Users/me/proj/src/app.ts');
	});

	it('does not double the separator of the root', () => {
		expect(projectEntryPath('/Users/me/proj/', 'README.md')).toBe('/Users/me/proj/README.md');
		expect(projectEntryPath('/', 'a')).toBe('/a');
	});

	it('uses backslashes for a Windows root', () => {
		expect(projectEntryPath('C:\\Projets\\app', 'src/main.rs')).toBe('C:\\Projets\\app\\src\\main.rs');
	});
});

describe('mentionKind: attachment kind of an existing entry', () => {
	it('is a folder block for a folder', () => {
		expect(mentionKind('/p/src', 'folder')).toBe('folder');
	});

	it('is an image for an image extension, whatever the case', () => {
		expect(mentionKind('/p/logo.PNG', 'file')).toBe('image');
	});

	it('is a file otherwise', () => {
		expect(mentionKind('/p/notes.md', 'file')).toBe('file');
	});
});
