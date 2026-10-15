// Pure display helpers for the settings dialog (no state, no IPC).

const integer = new Intl.NumberFormat('fr-FR');

/** Token counts, or « Inconnue » when the engine does not know the limit. */
export function formatTokens(count: number | null | undefined): string {
	return count == null ? 'Inconnue' : `${integer.format(count)} tokens`;
}

/** Plain count with French digit grouping. */
export function formatCount(count: number): string {
	return integer.format(count);
}

const MODALITY_LABELS: Record<string, string> = {
	text: 'Texte',
	image: 'Image',
	audio: 'Audio',
	video: 'Vidéo',
	pdf: 'PDF',
	file: 'Fichier'
};

/** Input modalities as a French list, unknown values shown as they are. */
export function formatModalities(input: readonly string[]): string {
	if (input.length === 0) return 'Texte';
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
