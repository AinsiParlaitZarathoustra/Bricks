// Labels of the Extensions section (extensions.ts): each Computer Use state and each permission
// state reads as the person sees it, in French; the tools offered count as « 1 outil » or « n outils ».
import { describe, expect, it } from 'vitest';
import { computerUseLabel, permissionLabel } from '../../src/lib/features/settings/extensions';
import type { ComputerUseStatus } from '../../src/lib/ipc/bindings/ComputerUseStatus';

function status(over: Partial<ComputerUseStatus> = {}): ComputerUseStatus {
	return {
		enabled: false,
		state: 'off',
		error: null,
		tools: 0,
		driver: null,
		accessibility: 'unknown',
		screen_recording: 'unknown',
		...over
	};
}

describe('computerUseLabel', () => {
	it.each([
		['unsupported', 'Indisponible sur ce système'],
		['missing', 'Cua Driver introuvable'],
		['off', 'Désactivé'],
		['starting', 'Démarrage…'],
		['error', 'Erreur']
	] as const)('reads « %s » as « %s »', (state, label) => {
		expect(computerUseLabel(status({ state }))).toBe(label);
	});

	it('names the tools offered when ready, with the singular for one', () => {
		expect(computerUseLabel(status({ state: 'ready', tools: 1 }))).toBe('Actif · 1 outil');
		expect(computerUseLabel(status({ state: 'ready', tools: 3 }))).toBe('Actif · 3 outils');
		expect(computerUseLabel(status({ state: 'ready', tools: 0 }))).toBe('Actif · 0 outils');
	});

	it('keeps the error text out of the label: it is shown on its own line', () => {
		expect(computerUseLabel(status({ state: 'error', error: 'le pilote ne répond pas' }))).toBe('Erreur');
	});
});

describe('permissionLabel', () => {
	it('says granted, missing and unknown in French', () => {
		expect(permissionLabel('granted')).toBe('Autorisée');
		expect(permissionLabel('missing')).toBe('Non autorisée');
		expect(permissionLabel('unknown')).toBe('Non vérifiée');
	});
});
