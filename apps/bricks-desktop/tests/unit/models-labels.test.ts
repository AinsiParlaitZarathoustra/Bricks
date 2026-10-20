// Unit tests for the labels, deletion texts and write-failure mapping of the models settings.
import { describe, expect, it } from 'vitest';
import type { ProviderView } from '../../src/lib/ipc/bindings/ProviderView';
import {
	CONFLICT_MESSAGE,
	authLabel,
	modelCountLabel,
	protocolLabel,
	providerDeletionDetail,
	writeFailure
} from '../../src/lib/features/settings/models';

function providerWith(names: string[]): ProviderView {
	return {
		id: 'acme',
		name: 'Acme',
		endpoint: 'https://api.acme.test',
		protocol: 'responses',
		auth: 'none',
		auth_header: null,
		api_key_env: null,
		has_inline_key: false,
		has_credentials: false,
		path: null,
		header_names: [],
		models: names.map((name, i) => ({
			id: `m${i}`,
			selection: `acme/m${i}`,
			name,
			api_model: name,
			protocol: null,
			endpoint: null,
			input_modalities: [],
			output_modalities: [],
			streaming: false,
			tool_calls: false,
			max_input_tokens: 1,
			max_output_tokens: 1,
			context_window_tokens: null,
			reasoning_default: null,
			profiles: [],
			priced: false,
			document_mime_types: [],
			pricing: null
		}))
	};
}

describe('protocol and auth labels', () => {
	it('names the known values and keeps unknown ones as they are', () => {
		expect(protocolLabel('anthropic_messages')).toBe('Anthropic Messages');
		expect(protocolLabel('exotic')).toBe('exotic');
		expect(authLabel('api_key_header')).toBe('Clé dans un en-tête');
		expect(authLabel('none')).toBe('Aucune');
	});
});

describe('modelCountLabel', () => {
	it('uses the singular for one model only', () => {
		expect(modelCountLabel(0)).toBe('0 modèles');
		expect(modelCountLabel(1)).toBe('1 modèle');
		expect(modelCountLabel(2)).toBe('2 modèles');
	});
});

describe('providerDeletionDetail', () => {
	it('names every model the provider takes with it', () => {
		const detail = providerDeletionDetail(providerWith(['Fast', 'Deep']));
		expect(detail).toContain('« Fast »');
		expect(detail).toContain('« Deep »');
	});

	it('says so when the provider has no model', () => {
		expect(providerDeletionDetail(providerWith([]))).toBe('Aucun modèle n’est déclaré pour ce fournisseur.');
	});
});

describe('writeFailure', () => {
	it('explains a stale revision and flags it as a conflict', () => {
		const error = Object.assign(new Error('revision périmée'), { code: 'conflict' });
		expect(writeFailure(error)).toEqual({ ok: false, message: CONFLICT_MESSAGE, conflict: true });
	});

	it('shows the engine message for any other failure', () => {
		const error = Object.assign(new Error('not implemented yet'), { code: 'internal' });
		expect(writeFailure(error)).toEqual({ ok: false, message: 'not implemented yet', conflict: false });
	});

	it('handles values that are not errors', () => {
		expect(writeFailure('boom')).toEqual({ ok: false, message: 'boom', conflict: false });
		expect(writeFailure(new Error(''))).toMatchObject({ ok: false, conflict: false });
	});
});
