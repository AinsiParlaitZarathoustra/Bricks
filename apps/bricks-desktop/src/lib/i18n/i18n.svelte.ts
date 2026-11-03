/**
 * The interface language of Bricks: `t()` for text, plural forms and local
 * formats. English is the default, French the other language.
 *
 * - The messages live in `messages/en.ts` and `messages/fr.ts`, under English
 *   keys; `fr.ts` must hold every key of `en.ts`.
 * - `t()` reads the language, so what a template or a `$derived` shows follows
 *   a change at once. A text computed once (a module constant) would not: such
 *   tables use getters.
 * - The choice is saved in the preferences (`app.prefs.language`); the language
 *   last shown (`localStorage`, a display copy) avoids a flash at start-up.
 * - Conversations, files and model output are never translated.
 */

import type { Language } from '$lib/ipc/bindings/Language';
import { en, type Message, type MessageKey } from './messages/en';
import { fr } from './messages/fr';

export type { Language, MessageKey };

export const LANGUAGES: ReadonlyArray<{ value: Language; label: string }> = [
	{ value: 'en', label: 'English' },
	{ value: 'fr', label: 'Français' }
];

const STORAGE_KEY = 'bricks.language';
const LOCALE_TAGS: Record<Language, string> = { en: 'en-US', fr: 'fr-FR' };
const CATALOGS: Record<Language, Record<MessageKey, Message>> = { en, fr };

export function isLanguage(value: unknown): value is Language {
	return value === 'en' || value === 'fr';
}

function readLastLanguage(): Language | null {
	try {
		const saved = globalThis.localStorage?.getItem(STORAGE_KEY);
		return isLanguage(saved) ? saved : null;
	} catch {
		return null;
	}
}

class I18nState {
	language = $state<Language>(readLastLanguage() ?? 'en');
}

const state = new I18nState();

function showLanguage(language: Language) {
	if (typeof document !== 'undefined') document.documentElement.lang = language;
}

showLanguage(state.language);

/** The language shown now. */
export function currentLanguage(): Language {
	return state.language;
}

/** The BCP 47 tag of the language shown, for `Intl` and `localeCompare`. */
export function localeTag(): string {
	return LOCALE_TAGS[state.language];
}

/** Shows `language` at once; saving it in the preferences is the caller's job. */
export function setLanguage(language: Language) {
	state.language = language;
	showLanguage(language);
	try {
		globalThis.localStorage?.setItem(STORAGE_KEY, language);
	} catch {
		// The display copy is a convenience: the preferences hold the choice.
	}
}

export type MessageParams = Record<string, string | number | null | undefined>;

/**
 * Remembers one `Intl` formatter per language and options. Numbers, dates and
 * relative times are built through it, so a formatter declared once in a module
 * follows the language.
 */
function perLanguage<F>(create: (tag: string) => F): () => F {
	const made = new Map<string, F>();
	return () => {
		const tag = localeTag();
		let formatter = made.get(tag);
		if (!formatter) {
			formatter = create(tag);
			made.set(tag, formatter);
		}
		return formatter;
	};
}

export function numberFormat(options?: Intl.NumberFormatOptions) {
	const formatter = perLanguage((tag) => new Intl.NumberFormat(tag, options));
	return { format: (value: number) => formatter().format(value) };
}

export function dateTimeFormat(options?: Intl.DateTimeFormatOptions) {
	const formatter = perLanguage((tag) => new Intl.DateTimeFormat(tag, options));
	return { format: (value: Date | number) => formatter().format(value) };
}

export function relativeTimeFormat(options?: Intl.RelativeTimeFormatOptions) {
	const formatter = perLanguage((tag) => new Intl.RelativeTimeFormat(tag, options));
	return {
		format: (value: number, unit: Intl.RelativeTimeFormatUnit) => formatter().format(value, unit)
	};
}

const pluralRules = perLanguage((tag) => new Intl.PluralRules(tag));
const integer = numberFormat();

function pick(message: Message, count: number): string {
	if (typeof message === 'string') return message;
	return pluralRules().select(count) === 'one' ? message.one : message.other;
}

/**
 * The text of `key` in the language shown, with its `{placeholders}` filled.
 * A message with plural forms chooses by `params.count`, shown in the local
 * number format.
 */
export function t(key: MessageKey, params?: MessageParams): string {
	const message = CATALOGS[state.language][key] ?? en[key];
	const count = Number(params?.count ?? 0);
	const text = pick(message, count);
	if (!params) return text;
	return text.replace(/\{(\w+)\}/g, (placeholder, name: string) => {
		if (!(name in params)) return placeholder;
		if (name === 'count' && Number.isFinite(count)) return integer.format(count);
		return String(params[name] ?? '');
	});
}
