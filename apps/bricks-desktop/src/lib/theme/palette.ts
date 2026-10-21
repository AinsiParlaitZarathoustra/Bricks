// Colour palette of Bricks: pure helpers, no runes and no DOM, so the appearance
// settings, the theme and the tests share one set of rules.
//
// Three choices are customised per resolved mode (light or dark):
//   - accent      -> --primary, --ring, --sidebar-primary (+ readable foregrounds)
//   - background  -> --background (central surface), with its text colours
//   - panel       -> --sidebar (navigation and side panels), with its text colours
// A role without a value keeps the theme token of src/app.css. Status, error and diff colours are
// never customised. Values are `#rrggbb` strings; `null` in the stored ColorSet means "theme token".

import type { ColorSet } from '$lib/ipc/bindings/ColorSet';

export type Hex = string;
export type ColorRole = 'accent' | 'background' | 'panel';
export type ResolvedMode = 'light' | 'dark';

export const COLOR_ROLES: readonly ColorRole[] = ['accent', 'background', 'panel'];

export const COLOR_ROLE_LABELS: Record<ColorRole, string> = {
	accent: 'Accent',
	background: 'Fond principal',
	panel: 'Fond des panneaux'
};

/** Text colours used on customised surfaces: near-black and near-white, never pure black or white. */
export const TEXT_DARK: Hex = '#1d1b18';
export const TEXT_LIGHT: Hex = '#fbfaf7';

// ─── Hex, RGB and HSV ────────────────────────────────────────────────────────

type Rgb = [number, number, number];

/** A `#rrggbb` (or `#rgb`, or without `#`) colour, in lower case; `null` when it is not a colour. */
export function normalizeHex(text: string): Hex | null {
	const body = text.trim().replace(/^#/, '');
	const full = /^[0-9a-f]{3}$/i.test(body) ? body.split('').map((c) => c + c).join('') : body;
	return /^[0-9a-f]{6}$/i.test(full) ? `#${full.toLowerCase()}` : null;
}

function validHex(value: unknown): Hex | null {
	return typeof value === 'string' ? normalizeHex(value) : null;
}

function hexToRgb(hex: Hex): Rgb {
	const n = parseInt(hex.slice(1), 16);
	return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

function clampChannel(value: number): number {
	return Math.round(Math.min(255, Math.max(0, value)));
}

function rgbToHex(rgb: Rgb): Hex {
	return `#${rgb.map((c) => clampChannel(c).toString(16).padStart(2, '0')).join('')}`;
}

export interface Hsv {
	/** Hue in degrees, [0, 360). */
	h: number;
	/** Saturation, [0, 1]. */
	s: number;
	/** Value (brightness), [0, 1]. */
	v: number;
}

export function hexToHsv(hex: Hex): Hsv {
	const [r0, g0, b0] = hexToRgb(hex);
	const r = r0 / 255;
	const g = g0 / 255;
	const b = b0 / 255;
	const max = Math.max(r, g, b);
	const min = Math.min(r, g, b);
	const d = max - min;
	let h = 0;
	if (d !== 0) {
		if (max === r) h = ((g - b) / d) % 6;
		else if (max === g) h = (b - r) / d + 2;
		else h = (r - g) / d + 4;
		h *= 60;
		if (h < 0) h += 360;
	}
	return { h, s: max === 0 ? 0 : d / max, v: max };
}

export function hsvToHex({ h, s, v }: Hsv): Hex {
	const hue = ((h % 360) + 360) % 360;
	const sat = Math.min(1, Math.max(0, s));
	const val = Math.min(1, Math.max(0, v));
	const c = val * sat;
	const x = c * (1 - Math.abs(((hue / 60) % 2) - 1));
	const m = val - c;
	let rgb: Rgb;
	if (hue < 60) rgb = [c, x, 0];
	else if (hue < 120) rgb = [x, c, 0];
	else if (hue < 180) rgb = [0, c, x];
	else if (hue < 240) rgb = [0, x, c];
	else if (hue < 300) rgb = [x, 0, c];
	else rgb = [c, 0, x];
	return rgbToHex([(rgb[0] + m) * 255, (rgb[1] + m) * 255, (rgb[2] + m) * 255]);
}

// ─── Contrast ────────────────────────────────────────────────────────────────

function linearChannel(c: number): number {
	const v = c / 255;
	return v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
}

/** WCAG relative luminance, [0, 1]. */
export function luminance(hex: Hex): number {
	const [r, g, b] = hexToRgb(hex);
	return 0.2126 * linearChannel(r) + 0.7152 * linearChannel(g) + 0.0722 * linearChannel(b);
}

/** WCAG contrast ratio between two colours, from 1 to 21. */
export function contrastRatio(a: Hex, b: Hex): number {
	const la = luminance(a);
	const lb = luminance(b);
	const [hi, lo] = la >= lb ? [la, lb] : [lb, la];
	return (hi + 0.05) / (lo + 0.05);
}

/** The text colour (dark or light) with the higher contrast on `background`. */
export function textOn(background: Hex): Hex {
	return contrastRatio(background, TEXT_DARK) >= contrastRatio(background, TEXT_LIGHT) ? TEXT_DARK : TEXT_LIGHT;
}

/** Linear sRGB mix: `weight` is the share of `b` (0 = `a`, 1 = `b`). */
export function mixHex(a: Hex, b: Hex, weight: number): Hex {
	const w = Math.min(1, Math.max(0, weight));
	const [ar, ag, ab] = hexToRgb(a);
	const [br, bg, bb] = hexToRgb(b);
	return rgbToHex([ar + (br - ar) * w, ag + (bg - ag) * w, ab + (bb - ab) * w]);
}

/**
 * Secondary text on `background`: the weakest mix of `text` towards the background that still
 * keeps 4.5:1. Falls back to `text` when no mix does.
 */
export function mutedTextOn(background: Hex, text: Hex): Hex {
	for (let weight = 0.6; weight > 0; weight -= 0.05) {
		const candidate = mixHex(text, background, weight);
		if (contrastRatio(candidate, background) >= 4.5) return candidate;
	}
	return text;
}

// ─── Theme tokens (defaults) ─────────────────────────────────────────────────

/** Björn Ottosson's OKLCH to sRGB, used once to turn the tokens of src/app.css into hex. */
export function oklchToHex(l: number, c: number, hueDegrees: number): Hex {
	const h = (hueDegrees * Math.PI) / 180;
	const a = c * Math.cos(h);
	const b = c * Math.sin(h);
	const l1 = l + 0.3963377774 * a + 0.2158037573 * b;
	const m1 = l - 0.1055613458 * a - 0.0638541728 * b;
	const s1 = l - 0.0894841775 * a - 1.291485548 * b;
	const L = l1 * l1 * l1;
	const M = m1 * m1 * m1;
	const S = s1 * s1 * s1;
	const linear: Rgb = [
		4.0767416621 * L - 3.3077115913 * M + 0.2309699292 * S,
		-1.2684380046 * L + 2.6097574011 * M - 0.3413193965 * S,
		-0.0041960863 * L - 0.7034186147 * M + 1.707614701 * S
	];
	return rgbToHex(linear.map(encodeChannel) as Rgb);
}

function encodeChannel(x: number): number {
	const v = Math.min(1, Math.max(0, x));
	return 255 * (v <= 0.0031308 ? 12.92 * v : 1.055 * v ** (1 / 2.4) - 0.055);
}

interface TokenColors {
	accent: Hex;
	accentForeground: Hex;
	background: Hex;
	foreground: Hex;
	mutedForeground: Hex;
	panel: Hex;
	panelForeground: Hex;
}

/** The values of src/app.css (`:root` and `.dark`), in hex. Keep in step with that file. */
export const TOKENS: Record<ResolvedMode, TokenColors> = {
	light: {
		accent: oklchToHex(0.52, 0.12, 38),
		accentForeground: oklchToHex(0.985, 0.005, 85),
		background: oklchToHex(0.985, 0.004, 85),
		foreground: oklchToHex(0.235, 0.012, 60),
		mutedForeground: oklchToHex(0.52, 0.012, 65),
		panel: oklchToHex(0.967, 0.007, 82),
		panelForeground: oklchToHex(0.235, 0.012, 60)
	},
	dark: {
		accent: oklchToHex(0.74, 0.12, 42),
		accentForeground: oklchToHex(0.2, 0.01, 50),
		background: oklchToHex(0.19, 0.006, 60),
		foreground: oklchToHex(0.94, 0.008, 80),
		mutedForeground: oklchToHex(0.74, 0.01, 75),
		panel: oklchToHex(0.205, 0.006, 65),
		panelForeground: oklchToHex(0.94, 0.008, 80)
	}
};

// ─── What a custom colour changes ────────────────────────────────────────────

/** Every CSS variable a custom colour can set. Cleared before each apply, so a reset restores the token. */
export const PALETTE_VARIABLES: readonly string[] = [
	'--primary',
	'--primary-foreground',
	'--ring',
	'--sidebar-primary',
	'--sidebar-primary-foreground',
	'--sidebar-ring',
	'--background',
	'--foreground',
	'--muted-foreground',
	'--card',
	'--card-foreground',
	'--popover',
	'--popover-foreground',
	'--muted',
	'--secondary',
	'--secondary-foreground',
	'--accent',
	'--accent-foreground',
	'--sidebar',
	'--sidebar-foreground',
	'--sidebar-accent',
	'--sidebar-accent-foreground',
	'--sidebar-border'
];

/**
 * The CSS variables that the custom colours of one mode set. Only the roles with a value appear.
 * Foreground and secondary colours are derived from the customised surface so that text stays
 * readable on it. Nothing here depends on the resolved mode: the stored set already belongs to it.
 */
export function paletteVariables(colors: ColorSet | null | undefined): Record<string, Hex> {
	const out: Record<string, Hex> = {};
	const accent = validHex(colors?.accent);
	const background = validHex(colors?.background);
	const panel = validHex(colors?.panel);

	if (accent) {
		const onAccent = textOn(accent);
		Object.assign(out, {
			'--primary': accent,
			'--ring': accent,
			'--sidebar-primary': accent,
			'--sidebar-ring': accent,
			'--primary-foreground': onAccent,
			'--sidebar-primary-foreground': onAccent
		});
	}

	if (background) {
		const text = textOn(background);
		const surface = (weight: number) => mixHex(background, text, weight);
		Object.assign(out, {
			'--background': background,
			'--foreground': text,
			'--muted-foreground': mutedTextOn(background, text),
			'--card': surface(0.035),
			'--card-foreground': text,
			'--popover': surface(0.035),
			'--popover-foreground': text,
			'--muted': surface(0.06),
			'--secondary': surface(0.08),
			'--secondary-foreground': text,
			'--accent': surface(0.1),
			'--accent-foreground': text
		});
	}

	if (panel) {
		const text = textOn(panel);
		Object.assign(out, {
			'--sidebar': panel,
			'--sidebar-foreground': text,
			'--sidebar-accent': mixHex(panel, text, 0.08),
			'--sidebar-accent-foreground': text,
			'--sidebar-border': mixHex(panel, text, 0.14)
		});
	}

	return out;
}

/** Sets the custom variables on `root` (inline, so they win over the stylesheet) and clears the others. */
export function applyPaletteVariables(root: HTMLElement, variables: Record<string, Hex>): void {
	for (const name of PALETTE_VARIABLES) {
		const value = variables[name];
		if (value) root.style.setProperty(name, value);
		else root.style.removeProperty(name);
	}
}

// ─── Values shown in the settings ────────────────────────────────────────────

/** The colour of a role for a mode: the custom value, else the theme token. */
export function roleColor(colors: ColorSet | null | undefined, mode: ResolvedMode, role: ColorRole): Hex {
	return validHex(colors?.[role]) ?? TOKENS[mode][role];
}

/** Whether a role has a custom value in this mode. */
export function isCustomRole(colors: ColorSet | null | undefined, role: ColorRole): boolean {
	return validHex(colors?.[role]) !== null;
}

/** The text colour shown on a role's surface (for the preview). */
export function roleForeground(colors: ColorSet | null | undefined, mode: ResolvedMode, role: ColorRole): Hex {
	if (role === 'accent') {
		const accent = validHex(colors?.accent);
		return accent ? textOn(accent) : TOKENS[mode].accentForeground;
	}
	const surface = validHex(colors?.[role]);
	if (surface) return textOn(surface);
	return role === 'background' ? TOKENS[mode].foreground : TOKENS[mode].panelForeground;
}

export interface ColorCheck {
	label: string;
	ratio: number;
	/** The WCAG threshold this check must reach. */
	min: number;
}

/** Contrast checks of a role, against the surfaces it sits on. Warnings only: the choice is never blocked. */
export function colorChecks(colors: ColorSet | null | undefined, mode: ResolvedMode, role: ColorRole): ColorCheck[] {
	const background = roleColor(colors, mode, 'background');
	const panel = roleColor(colors, mode, 'panel');
	const accent = roleColor(colors, mode, 'accent');
	if (role === 'accent') {
		return [
			{ label: 'Focus sur le fond principal', ratio: contrastRatio(accent, background), min: 3 },
			{ label: 'Texte sur l’accent', ratio: contrastRatio(roleForeground(colors, mode, 'accent'), accent), min: 4.5 }
		];
	}
	if (role === 'background') {
		return [
			{
				label: 'Texte sur le fond principal',
				ratio: contrastRatio(roleForeground(colors, mode, 'background'), background),
				min: 4.5
			}
		];
	}
	return [
		{
			label: 'Texte sur les panneaux',
			ratio: contrastRatio(roleForeground(colors, mode, 'panel'), panel),
			min: 4.5
		}
	];
}

// ─── Presets ─────────────────────────────────────────────────────────────────

export interface Swatch {
	label: string;
	hex: Hex;
}

/** Quick choices of each role, in French. A custom colour is still possible from the palette. */
export const PRESETS: Record<ColorRole, readonly Swatch[]> = {
	accent: [
		{ label: 'Terre cuite', hex: '#a4512c' },
		{ label: 'Ambre', hex: '#b7791f' },
		{ label: 'Olive', hex: '#5f7a2b' },
		{ label: 'Vert', hex: '#2f7d5b' },
		{ label: 'Sarcelle', hex: '#1f7a80' },
		{ label: 'Bleu', hex: '#2f5fa8' },
		{ label: 'Indigo', hex: '#4c4fb3' },
		{ label: 'Violet', hex: '#7b4bb0' },
		{ label: 'Rose', hex: '#b03f74' },
		{ label: 'Graphite', hex: '#4a4f57' }
	],
	background: [
		{ label: 'Papier', hex: '#fbfaf7' },
		{ label: 'Sable', hex: '#f4efe6' },
		{ label: 'Brume', hex: '#eef1f4' },
		{ label: 'Sauge', hex: '#eef2ec' },
		{ label: 'Charbon', hex: '#1c1b19' },
		{ label: 'Nuit', hex: '#14171c' }
	],
	panel: [
		{ label: 'Lin', hex: '#f0ebe1' },
		{ label: 'Bruine', hex: '#e8e4dc' },
		{ label: 'Ardoise claire', hex: '#e6e9ee' },
		{ label: 'Sauge claire', hex: '#e8efe9' },
		{ label: 'Cacao', hex: '#2a2622' },
		{ label: 'Ardoise', hex: '#262a31' }
	]
};
