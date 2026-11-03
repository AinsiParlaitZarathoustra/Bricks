// Pure display helpers for the settings dialog (no state, no IPC).
import { t, numberFormat } from '$lib/i18n/i18n.svelte';


const integer = numberFormat();

/** Token counts, or « Inconnue » when the engine does not know the limit. */
export function formatTokens(count: number | null | undefined): string {
	return count == null ? t('settings.unknown') : t('common.tokens', { tokens: integer.format(count) });
}

/** Plain count with French digit grouping. */
export function formatCount(count: number): string {
	return integer.format(count);
}

export const MODALITY_LABELS: Record<string, string> = {
	get text() { return t('settings.text') },
	get image() { return t('settings.image') },
	get audio() { return t('settings.audio') },
	get video() { return t('settings.video') },
	get document() { return t('settings.document') }
};

const price = numberFormat({ minimumFractionDigits: 2, maximumFractionDigits: 12 });

/** A price per million tokens, « 0,30 $ », or « — » when it is not in the file. */
export function formatPrice(value: string | null | undefined): string {
	return value == null ? '—' : `${price.format(Number(value))} $`;
}

/** Input modalities as a French list, unknown values shown as they are. Nothing declared says so. */
export function formatModalities(input: readonly string[]): string {
	if (input.length === 0) return t('settings.notDeclared');
	return input.map((m) => MODALITY_LABELS[m] ?? m).join(', ');
}

const PLATFORM_LABELS: Record<string, string> = {
	macos: 'macOS',
	darwin: 'macOS',
	windows: 'Windows',
	linux: 'Linux'
};

/** Operating system name for the application section. */
export function platformLabel(platform: string): string {
	return PLATFORM_LABELS[platform.toLowerCase()] ?? platform;
}

/** Path of the providers file inside the Bricks home folder (display only, no I/O). */
export function providersFilePath(bricksHome: string): string {
	const sep = bricksHome.includes('\\') && !bricksHome.includes('/') ? '\\' : '/';
	return `${bricksHome.replace(/[\\/]+$/, '')}${sep}providers.toml`;
}

/** `1.0.0-alpha.1` as people read it: `Bricks 1.0 Alpha`. */
export function versionLabel(version: string): string {
	const parts = /^(\d+)\.(\d+)\.\d+(?:-([a-z]+))?/i.exec(version);
	if (!parts) return `Bricks ${version}`;
	const [, major, minor, stage] = parts;
	const suffix = stage ? ` ${stage[0].toUpperCase()}${stage.slice(1)}` : '';
	return `Bricks ${major}.${minor}${suffix}`;
}
