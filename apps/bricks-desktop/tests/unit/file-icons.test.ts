// @vitest-environment happy-dom
// File, folder and language icons (resolve.ts) against the catalog it names: the rules, the
// SVG files and their manifest, the license files, and hostile inputs, which are data only: a
// URL only ever names a local catalog file.
import { createHash } from 'node:crypto';
import { existsSync, readFileSync, readdirSync, statSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import {
	baseName,
	fileIconId,
	folderIconId,
	iconId,
	iconImgHtml,
	iconUrl,
	isIconId,
	languageIconId
} from '../../src/lib/features/file-icons/resolve';

interface Rules {
	generic: string;
	folder: { closed: string; open: string; root: string };
	exact: Record<string, string>;
	extensions: Record<string, string>;
	folders: Record<string, string>;
	aliases: Record<string, string>;
	icons: Record<string, string>;
}

// The npm scripts run from apps/bricks-desktop (happy-dom gives no file URL to resolve from).
const APP = `${process.cwd()}/`;
const ICONS_DIR = `${APP}public/file-icons/`;
const RULES_PATH = `${APP}src/lib/features/file-icons/rules.json`;
const rules = JSON.parse(readFileSync(RULES_PATH, 'utf8')) as Rules;
const manifest = JSON.parse(readFileSync(`${ICONS_DIR}manifest.json`, 'utf8')) as {
	rules: unknown;
	icons: { id: string; file: string; source: string; license: unknown; sha256: string }[];
};

const GENERIC_URL = /\/file-icons\/svg\/file-generic\.svg$/;

/** The icon ids the rules name, directly or through an alias. */
function namedIds(): Set<string> {
	const ids = new Set<string>([rules.generic, rules.folder.closed, rules.folder.open, rules.folder.root]);
	for (const id of [
		...Object.values(rules.exact),
		...Object.values(rules.extensions),
		...Object.values(rules.folders)
	]) ids.add(id);
	for (const label of Object.keys(rules.aliases)) {
		const id = languageIconId(label);
		if (id !== null) ids.add(id);
	}
	return ids;
}

describe('files: exact names, then the longest suffix, then the generic icon', () => {
	it.each(['Cargo.toml', 'cargo.toml', 'CARGO.TOML'])('%s is the Cargo icon', (name) => {
		expect(fileIconId(name)).toBe('file-cargo-toml');
	});

	it('an exact name beats the suffix it ends with', () => {
		expect(fileIconId('package.json')).toBe('file-package-json');
		expect(fileIconId('other.json')).toBe('fmt-json');
		expect(fileIconId('Cargo.toml')).not.toBe(fileIconId('other.toml'));
	});

	it('dotfiles are matched by their exact name', () => {
		expect(fileIconId('.gitignore')).toBe('file-gitignore');
		expect(fileIconId('.env.example')).toBe('fmt-tune');
		expect(fileIconId('/repo/.env.example')).toBe('fmt-tune');
	});

	it('the longest compound suffix wins over the simple one', () => {
		expect(fileIconId('archive.schema.json')).toBe(rules.extensions['schema.json']);
		expect(fileIconId('archive.schema.json')).not.toBe(rules.extensions['json']);
		expect(fileIconId('App.test.tsx')).toBe(rules.extensions['test.tsx']);
		expect(fileIconId('App.test.tsx')).not.toBe(rules.extensions['tsx']);
	});

	it('a simple extension names its language', () => {
		expect(fileIconId('main.rs')).toBe('rust');
		expect(fileIconId('index.ts')).toBe('typescript');
		expect(fileIconId('app.js')).toBe('javascript');
		expect(fileIconId('script.py')).toBe('python');
		expect(fileIconId('App.tsx')).toBe(rules.extensions['tsx']);
	});

	it('an unknown extension is the generic icon', () => {
		expect(fileIconId('weird.qqq')).toBe(rules.generic);
		expect(fileIconId('weird.qqq')).toBe('file-generic');
	});

	it('a name without an extension: LICENSE is exact, notes is generic', () => {
		expect(fileIconId('LICENSE')).toBe('file-license');
		expect(fileIconId('notes')).toBe(rules.generic);
	});

	it('the empty name is generic', () => {
		expect(fileIconId('')).toBe(rules.generic);
	});
});

describe('paths: Windows and Unix separators name the same file', () => {
	it('a Windows path and a Unix path give the same icon', () => {
		expect(fileIconId('C:\\proj\\src\\main.rs')).toBe('rust');
		expect(fileIconId('/a/b/main.rs')).toBe('rust');
		expect(fileIconId('a\\b\\Cargo.toml')).toBe('file-cargo-toml');
	});

	it('baseName keeps the last segment, whatever the separators', () => {
		expect(baseName('C:\\x\\y.rs')).toBe('y.rs');
		expect(baseName('/a/b/')).toBe('b');
		expect(baseName('plain.md')).toBe('plain.md');
	});

	it('a folder path ends with a separator and keeps its name', () => {
		expect(folderIconId('C:\\proj\\src\\')).toBe(rules.folders['src']);
		expect(folderIconId('/a/b/tests/')).toBe(rules.folders['tests']);
	});
});

describe('folders', () => {
	it('a named folder has its own icon, case-insensitively', () => {
		expect(folderIconId('src')).toBe(rules.folders['src']);
		expect(folderIconId('SRC')).toBe(rules.folders['src']);
		expect(folderIconId('tests')).toBe(rules.folders['tests']);
		expect(folderIconId('node_modules')).toBe('folder-folder-node');
		expect(folderIconId('Node_Modules')).toBe('folder-folder-node');
	});

	it('an unnamed folder is the closed or the open generic icon', () => {
		expect(folderIconId('lib')).toBe(rules.folder.closed);
		expect(folderIconId('lib', false)).toBe('folder-closed');
		expect(folderIconId('lib', true)).toBe(rules.folder.open);
	});

	it('a named folder keeps its icon when it is open', () => {
		expect(folderIconId('src', true)).toBe(rules.folders['src']);
		expect(iconId({ kind: 'folder', path: 'tests', open: true })).toBe(rules.folders['tests']);
	});

	it('the root has its own icon', () => {
		expect(iconId({ kind: 'root', path: '/work/demo' })).toBe('folder-root');
		expect(folderIconId('', false, true)).toBe('folder-root');
	});
});

describe('language labels (code fences) and their aliases', () => {
	it.each([
		['rust', 'rs'],
		['typescript', 'ts'],
		['javascript', 'js'],
		['python', 'py']
	])('%s and %s give the same icon, the one of their file', (long, short) => {
		expect(languageIconId(long)).not.toBeNull();
		expect(languageIconId(long)).toBe(languageIconId(short));
	});

	it('rust and rs name the rust icon', () => {
		expect(languageIconId('rust')).toBe('rust');
		expect(languageIconId('rs')).toBe('rust');
		expect(iconId({ language: 'rust' })).toBe('rust');
	});

	it('tsx and jsx name the icon of their extension', () => {
		expect(languageIconId('tsx')).toBe(fileIconId('x.tsx'));
		expect(languageIconId('jsx')).toBe(fileIconId('x.jsx'));
	});

	it('an unknown label is null, and a missing label too', () => {
		expect(languageIconId('brainfuck')).toBeNull();
		expect(languageIconId('')).toBeNull();
		expect(languageIconId(null)).toBeNull();
		expect(languageIconId(undefined)).toBeNull();
	});

	it('iconId without a path uses the language, and the generic icon without either', () => {
		expect(iconId({ language: 'python' })).toBe(fileIconId('x.py'));
		expect(iconId({ language: 'brainfuck' })).toBe(rules.generic);
		expect(iconId({})).toBe(rules.generic);
	});

	it('a known path wins over a language that contradicts it', () => {
		expect(iconId({ path: 'notes.txt', language: 'rust' })).toBe(fileIconId('notes.txt'));
		expect(iconId({ path: 'notes.txt', language: 'rust' })).not.toBe('rust');
		expect(iconId({ path: 'main.rs', language: 'python' })).toBe('rust');
	});

	it('every alias names an icon of the catalog', () => {
		const missing = Object.keys(rules.aliases).filter((label) => {
			const id = languageIconId(label);
			return id === null || !Object.hasOwn(rules.icons, id);
		});
		expect(missing).toEqual([]);
	});
});

describe('assets: the rules name only icons the catalog has, and the catalog has its files', () => {
	it('every id the rules name is an icon of rules.icons', () => {
		const unknown = [...namedIds()].filter((id) => !Object.hasOwn(rules.icons, id));
		expect(unknown).toEqual([]);
		for (const id of namedIds()) expect(isIconId(id)).toBe(true);
	});

	it('every icon of rules.icons is a file under public/file-icons', () => {
		const missing = Object.entries(rules.icons)
			.filter(([, file]) => !existsSync(`${ICONS_DIR}${file}`) || !statSync(`${ICONS_DIR}${file}`).isFile())
			.map(([id]) => id);
		expect(missing).toEqual([]);
	});

	it('every SVG parses as an SVG document', () => {
		const parser = new DOMParser();
		const broken = Object.entries(rules.icons).filter(([, file]) => {
			const doc = parser.parseFromString(readFileSync(`${ICONS_DIR}${file}`, 'utf8'), 'image/svg+xml');
			return doc.getElementsByTagName('parsererror').length > 0 || doc.documentElement.localName !== 'svg';
		});
		expect(broken.map(([id]) => id)).toEqual([]);
	});

	it('no SVG carries script, event handlers, remote references or entities', () => {
		const hostile = [
			/<script/i,
			/\son[a-z]+\s*=/i,
			/\bhref\s*=\s*["']?\s*https?:/i,
			/xlink:href\s*=\s*["']?\s*https?:/i,
			/javascript:/i,
			/<!ENTITY/i,
			/<!DOCTYPE/i,
			/@import/i
		];
		const offenders = Object.entries(rules.icons)
			.filter(([, file]) => {
				const text = readFileSync(`${ICONS_DIR}${file}`, 'utf8');
				return hostile.some((pattern) => pattern.test(text));
			})
			.map(([id]) => id);
		expect(offenders).toEqual([]);
	});

	it('the manifest lists every icon, with its checksum, source and license', () => {
		const listed = new Map(manifest.icons.map((entry) => [entry.id, entry]));
		expect([...listed.keys()].sort()).toEqual(Object.keys(rules.icons).sort());
		for (const [id, file] of Object.entries(rules.icons)) {
			const entry = listed.get(id);
			expect(entry?.file, id).toBe(file);
			expect(entry?.source, id).toMatch(/^https:\/\//);
			expect(entry?.license, id).toBeTruthy();
			const digest = createHash('sha256').update(readFileSync(`${ICONS_DIR}${file}`)).digest('hex');
			expect(entry?.sha256, id).toBe(digest);
		}
	});

	it('the manifest rules are the rules of rules.json', () => {
		expect(manifest.rules).toEqual(rules);
	});

	it('the license files and the notice exist', () => {
		for (const name of [
			'LICENSE-DEVICON.txt',
			'LICENSE-MATERIAL-ICON-THEME.txt',
			'LICENSE-VSCODE-ICONS.txt',
			'LICENSE-SIMPLE-ICONS.md',
			'NOTICE.md'
		]) {
			expect(existsSync(`${ICONS_DIR}${name}`), name).toBe(true);
			expect(readFileSync(`${ICONS_DIR}${name}`, 'utf8').trim().length, name).toBeGreaterThan(0);
		}
	});

	it('the svg folder holds only the catalog files (no stray asset)', () => {
		const files = new Set(Object.values(rules.icons));
		const onDisk = readdirSync(`${ICONS_DIR}svg`).map((name) => `svg/${name}`);
		expect(onDisk.filter((file) => !files.has(file))).toEqual([]);
	});
});

describe('hostile inputs are data: they never name a file outside the catalog', () => {
	it.each([
		'../../etc/passwd',
		'https://evil.example/x.svg',
		'javascript:alert(1)',
		'data:image/svg+xml,<svg onload="alert(1)"/>',
		'//evil.example/x.svg',
		''
	])('iconUrl(%j) is the generic icon under file-icons/', (hostile) => {
		const url = iconUrl(hostile);
		expect(url).toMatch(GENERIC_URL);
		expect(url).toMatch(/\/file-icons\//);
		expect(url).not.toMatch(/evil|etc|javascript|data:/);
	});

	it('every catalog id gives a local URL under file-icons/svg/', () => {
		for (const id of Object.keys(rules.icons)) {
			expect(iconUrl(id)).toMatch(/\/file-icons\/svg\/[^/]+\.svg$/);
		}
	});

	it('iconImgHtml names only a file-icons svg, and carries no handler', () => {
		const html = iconImgHtml('rust');
		expect(html).toMatch(/^<img class="file-icon" src="\/file-icons\/svg\/rust\.svg"/);
		expect(html).not.toMatch(/\son[a-z]+=/i);
		const hostile = iconImgHtml('../../x"onerror="alert(1)');
		expect(hostile).toMatch(/src="[^"]*\/file-icons\/svg\/file-generic\.svg"/);
		expect(hostile).not.toMatch(/onerror/);
	});

	it('a path to a remote or a parent folder resolves by its basename only, and stays local', () => {
		expect(fileIconId('https://evil.example/a.rs')).toBe('rust');
		expect(fileIconId('../../x.py')).toBe('python');
		for (const path of ['https://evil.example/a.rs', '../../x.py']) {
			const url = iconUrl(fileIconId(path));
			expect(url).toMatch(/\/file-icons\/svg\/[^/]+\.svg$/);
			expect(url).not.toMatch(/evil|\.\./);
		}
	});

	it('an id that is not a catalog id is not an icon', () => {
		expect(isIconId('../../etc/passwd')).toBe(false);
		expect(isIconId('https://evil.example/x.svg')).toBe(false);
		expect(isIconId('rust')).toBe(true);
	});
});

describe('names inherited from Object.prototype are not icon names', () => {
	// Found by probing: the rules are plain objects, so `constructor`, `__proto__` and
	// `toString` are found through the prototype chain. Expected: the generic icon, or
	// null for a label; actual: a function or object turned into text, or a TypeError.
	it('iconUrl(constructor) gives the generic icon URL (actual: "/file-icons/function Object() { [native code] }")', () => {
		expect(iconUrl('constructor')).toMatch(GENERIC_URL);
		expect(iconUrl('__proto__')).toMatch(GENERIC_URL);
		expect(iconImgHtml('toString')).toMatch(/file-icons\/svg\/file-generic\.svg/);
	});

	it('a label named constructor or __proto__ is an unknown label, not a TypeError (actual: "path.replace is not a function")', () => {
		expect(languageIconId('constructor')).toBeNull();
		expect(languageIconId('__proto__')).toBeNull();
		expect(iconId({ language: 'toString' })).toBe(rules.generic);
	});

	it('a file named a.constructor or constructor has an icon id string (actual: a function)', () => {
		expect(typeof fileIconId('a.constructor')).toBe('string');
		expect(typeof fileIconId('constructor')).toBe('string');
		expect(typeof folderIconId('constructor')).toBe('string');
	});
});
