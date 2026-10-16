// Unit tests for the display fallbacks of the models settings: an empty label shows the id, and a
// model with no declared modality says so instead of stating a capability.
import { describe, expect, it } from 'vitest';
import type { ProviderView } from '../../src/lib/ipc/bindings/ProviderView';
import { formatModalities } from '../../src/lib/features/settings/format';
import {
	CONFLICT_MESSAGE,
	displayName,
	profileLabel,
	providerDeletionDetail
} from '../../src/lib/features/settings/models';

describe('displayName', () => {
	it('shows the name when it has text', () => {
		expect(displayName('Fast', 'fast')).toBe('Fast');
		expect(displayName('  Fast  ', 'fast')).toBe('Fast');
	});

	it('falls back to the id for an empty or missing name', () => {
		expect(displayName('', 'fast')).toBe('fast');
		expect(displayName('   ', 'fast')).toBe('fast');
		expect(displayName(null, 'fast')).toBe('fast');
		expect(displayName(undefined, 'fast')).toBe('fast');
	});
});

describe('profileLabel', () => {
	it('uses the label, or the id when the label is empty or missing', () => {
		expect(profileLabel({ id: 'high', label: 'Approfondi' })).toBe('Approfondi');
		expect(profileLabel({ id: 'high', label: '' })).toBe('high');
		expect(profileLabel({ id: 'high', label: null })).toBe('high');
	});
});

describe('formatModalities with nothing declared', () => {
	it('says that no modality is declared, without claiming text', () => {
		expect(formatModalities([])).toBe('Non déclarées');
		expect(formatModalities(['image'])).toBe('Image');
	});
});

describe('deletion text with unnamed models', () => {
	it('names an unnamed model by its id', () => {
		const provider: ProviderView = {
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
			models: [
				{
					id: 'mystery',
					selection: 'acme/mystery',
					name: ' ',
					api_model: 'mystery',
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
					priced: false
				}
			]
		};
		expect(providerDeletionDetail(provider)).toContain('« mystery »');
	});
});

describe('conflict notice', () => {
	it('says the changes were not saved and the values were reloaded', () => {
		expect(CONFLICT_MESSAGE).toContain('modifié ailleurs');
		expect(CONFLICT_MESSAGE).toContain('pas été enregistrées');
	});
});
