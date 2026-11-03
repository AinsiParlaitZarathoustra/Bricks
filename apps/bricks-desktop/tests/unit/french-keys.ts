// Source-reading tests check that the interface shows a text by its catalogue key: the
// French text is looked up in messages/fr.ts, and the source must call `t('<key>')` for it.
// A French text that is absent from the catalogue, or present under two keys, fails here.
import { fr } from '../../src/lib/i18n/messages/fr';

type Entry = string | { one: string; other: string };

/** The catalogue key whose French text is `text` (a singular or plural form). */
export function keyOf(text: string): string {
	const keys = Object.entries(fr as Record<string, Entry>)
		.filter(([, value]) => (typeof value === 'string' ? value === text : value.one === text || value.other === text))
		.map(([key]) => key);
	if (keys.length !== 1) throw new Error(`${keys.length} catalogue keys for « ${text} »`);
	return keys[0];
}

/** The source of a `t()` call for the French text, as the component writes it: `t('key')`. */
export function call(text: string): string {
	return `t('${keyOf(text)}')`;
}

/** An attribute bound to `t()`, as the component writes it: `attr={t('key')}`. */
export function bound(attr: string, text: string): string {
	return `${attr}={${call(text)}}`;
}

/** The start of a `t()` call with parameters after it: `t('key'` (the caller writes the rest). */
export function tkey(text: string): string {
	return `t('${keyOf(text)}'`;
}
