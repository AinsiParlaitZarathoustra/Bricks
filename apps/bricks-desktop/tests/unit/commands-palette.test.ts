// Palette logic: which `/` commands match a draft, completion, highlight moves.
// The registry here is a fixture shaped like the engine's list (names, aliases, args).
import { describe, expect, it } from 'vitest';
import type { SlashCommandInfo } from '../../src/lib/ipc/bindings/SlashCommandInfo';
import {
	completeLine,
	filterCommands,
	isExactName,
	moveIndex,
	paletteQuery,
	usageLine
} from '../../src/lib/features/commands/palette';

const REGISTRY: SlashCommandInfo[] = [
	{ name: 'model', aliases: ['m'], args: '[provider/model [profile]]', description: 'modèle' },
	{ name: 'effort', aliases: [], args: '[profile id | default]', description: 'effort' },
	{ name: 'search', aliases: ['s'], args: '<text> | re:<regex>', description: 'recherche' },
	{ name: 'agents', aliases: [], args: '[search words] | reload', description: 'agents' },
	{ name: 'skill', aliases: [], args: '<name> <request>', description: 'skill' },
	{ name: 'skills', aliases: [], args: '[search words]', description: 'skills' },
	{ name: 'changes', aliases: [], args: '<changeset id> inspect', description: 'changes' },
	{ name: 'jobs', aliases: [], args: '[stop <job id>]', description: 'jobs' },
	{ name: 'memory', aliases: [], args: '', description: 'mémoire' },
	{ name: 'context', aliases: ['ctx'], args: '', description: 'contexte' },
	{ name: 'cost', aliases: ['usage'], args: '', description: 'coût' },
	{ name: 'compact', aliases: [], args: '', description: 'compacter' },
	{ name: 'clear', aliases: [], args: '', description: 'vider' },
	{ name: 'help', aliases: ['?'], args: '', description: 'aide' },
	{ name: 'quit', aliases: ['exit', 'q'], args: '', description: 'quitter' }
];

const names = (list: SlashCommandInfo[]) => list.map((c) => c.name);

describe('paletteQuery', () => {
	it('is the typed name of a bare command line', () => {
		expect(paletteQuery('/')).toBe('');
		expect(paletteQuery('/comp')).toBe('comp');
		expect(paletteQuery('/?')).toBe('?');
	});

	it('is null once the line has arguments, a line break, or no slash', () => {
		expect(paletteQuery('/skill review')).toBeNull();
		expect(paletteQuery('/search ')).toBeNull();
		expect(paletteQuery('/a\nb')).toBeNull();
		expect(paletteQuery('bonjour')).toBeNull();
		expect(paletteQuery(' /memory')).toBeNull();
	});
});

describe('filterCommands', () => {
	it('lists every command, in registry order, for an empty name', () => {
		expect(names(filterCommands(REGISTRY, ''))).toEqual(REGISTRY.map((c) => c.name));
	});

	it('puts an exact alias first', () => {
		expect(names(filterCommands(REGISTRY, 'm'))[0]).toBe('model');
		expect(names(filterCommands(REGISTRY, 'ctx'))[0]).toBe('context');
		expect(names(filterCommands(REGISTRY, '?'))).toEqual(['help']);
		expect(names(filterCommands(REGISTRY, 'q'))[0]).toBe('quit');
	});

	it('ranks an exact name above name prefixes, and name prefixes above alias prefixes', () => {
		// `skill` is exact, `skills` is a name prefix.
		expect(names(filterCommands(REGISTRY, 'skill'))).toEqual(['skill', 'skills']);
		// `cost` is a name prefix of `co` (with `compact` and `context`); `usage` is not matched.
		expect(names(filterCommands(REGISTRY, 'co'))).toEqual(['context', 'cost', 'compact']);
		// `us` only matches the alias `usage` of cost.
		expect(names(filterCommands(REGISTRY, 'us'))).toEqual(['cost']);
	});

	it('matches case-insensitively and returns nothing for an unknown name', () => {
		expect(names(filterCommands(REGISTRY, 'MEM'))).toEqual(['memory']);
		expect(filterCommands(REGISTRY, 'zzz')).toEqual([]);
	});
});

describe('isExactName', () => {
	const model = REGISTRY[0];
	it('accepts the name and the aliases only', () => {
		expect(isExactName('model', model)).toBe(true);
		expect(isExactName('m', model)).toBe(true);
		expect(isExactName('M', model)).toBe(true);
		expect(isExactName('mo', model)).toBe(false);
	});
});

describe('completeLine and usageLine', () => {
	it('adds a space only when the command takes arguments', () => {
		expect(completeLine(REGISTRY[4])).toBe('/skill ');
		expect(completeLine(REGISTRY[8])).toBe('/memory');
	});

	it('shows the usage with the argument syntax', () => {
		expect(usageLine(REGISTRY[4])).toBe('/skill <name> <request>');
		expect(usageLine(REGISTRY[8])).toBe('/memory');
	});
});

describe('moveIndex', () => {
	it('moves and wraps around in both directions', () => {
		expect(moveIndex(0, 1, 3)).toBe(1);
		expect(moveIndex(2, 1, 3)).toBe(0);
		expect(moveIndex(0, -1, 3)).toBe(2);
	});

	it('stays at 0 when there is nothing to highlight', () => {
		expect(moveIndex(0, 1, 0)).toBe(0);
		expect(moveIndex(0, -1, 0)).toBe(0);
	});
});
