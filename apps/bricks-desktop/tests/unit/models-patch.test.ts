// Unit tests for the provider/model forms: validation and patch building.
import { describe, expect, it } from 'vitest';
import type { ModelView } from '../../src/lib/ipc/bindings/ModelView';
import type { ProviderView } from '../../src/lib/ipc/bindings/ProviderView';
import {
	buildModelPatch,
	buildProviderPatch,
	hasFormErrors,
	isEmptyModelPatch,
	isEmptyProviderPatch,
	modelForm,
	parseCount,
	providerForm,
	validateModelForm
} from '../../src/lib/features/settings/models';

const provider: ProviderView = {
	id: 'acme',
	name: 'Acme',
	endpoint: 'https://api.acme.test/v1',
	protocol: 'chat_completions',
	auth: 'bearer',
	auth_header: null,
	api_key_env: 'ACME_API_KEY',
	has_inline_key: false,
	has_credentials: true,
	path: null,
	header_names: [],
	models: []
};

const model: ModelView = {
	id: 'fast',
	selection: 'acme/fast',
	name: 'Fast',
	api_model: 'acme-fast-1',
	protocol: null,
	endpoint: null,
	input_modalities: ['text', 'image'],
	output_modalities: ['text'],
	streaming: true,
	tool_calls: true,
	max_input_tokens: 128000,
	max_output_tokens: 8192,
	context_window_tokens: 200000,
	reasoning_default: 'low',
	profiles: [
		{ id: 'low', label: 'Léger', parameters: {}, remove: [] },
		{ id: 'high', label: 'Approfondi', parameters: {}, remove: [] }
	],
	priced: true
};

describe('parseCount', () => {
	it('accepts whole numbers above zero, with grouping spaces', () => {
		expect(parseCount('8192')).toBe(8192);
		expect(parseCount(' 200 000 ')).toBe(200000);
		expect(parseCount('200_000')).toBe(200000);
	});

	it('refuses zero, negatives, decimals and text', () => {
		expect(parseCount('0')).toBeNull();
		expect(parseCount('-3')).toBeNull();
		expect(parseCount('1.5')).toBeNull();
		expect(parseCount('abc')).toBeNull();
		expect(parseCount('')).toBeNull();
	});
});

describe('validateModelForm', () => {
	it('requires both token limits to be above zero', () => {
		const errors = validateModelForm({ ...modelForm(model), maxInput: '0', maxOutput: '' });
		expect(errors.maxInput).toBeDefined();
		expect(errors.maxOutput).toBeDefined();
		expect(hasFormErrors(errors)).toBe(true);
	});

	it('has no context window field to validate', () => {
		const form = modelForm(model);
		expect(form).not.toHaveProperty('contextWindow');
		expect(hasFormErrors(validateModelForm(form))).toBe(false);
	});

	it('leaves a valid form without errors', () => {
		expect(hasFormErrors(validateModelForm(modelForm(model)))).toBe(false);
	});
});

describe('buildProviderPatch', () => {
	it('sends nothing when the form is untouched', () => {
		const patch = buildProviderPatch(provider, providerForm(provider));
		expect(patch).toEqual({
			name: null,
			endpoint: null,
			protocol: null,
			auth: null,
			auth_header: null,
			api_key_env: null
		});
		expect(isEmptyProviderPatch(patch)).toBe(true);
	});

	it('sends only the fields that changed, trimmed', () => {
		const form = { ...providerForm(provider), name: '  Acme Cloud ', endpoint: 'https://api.acme.test/v2' };
		expect(buildProviderPatch(provider, form)).toMatchObject({
			name: 'Acme Cloud',
			endpoint: 'https://api.acme.test/v2',
			protocol: null,
			auth: null
		});
	});

	it('treats an empty header and a missing header as the same value', () => {
		const form = { ...providerForm(provider), authHeader: '' };
		expect(buildProviderPatch(provider, form).auth_header).toBeNull();
		expect(buildProviderPatch(provider, { ...form, authHeader: 'X-Key' }).auth_header).toBe('X-Key');
	});

	it('leaves the variable name alone on a provider with an inline key', () => {
		const inline = { ...provider, has_inline_key: true, api_key_env: null };
		const form = { ...providerForm(inline), apiKeyEnv: 'OTHER_KEY' };
		expect(buildProviderPatch(inline, form).api_key_env).toBeNull();
	});

	it('sends a changed variable name', () => {
		const form = { ...providerForm(provider), apiKeyEnv: 'ACME_KEY_2' };
		expect(buildProviderPatch(provider, form).api_key_env).toBe('ACME_KEY_2');
	});
});

describe('buildModelPatch', () => {
	it('sends nothing when the form is untouched', () => {
		const patch = buildModelPatch(model, modelForm(model));
		expect(isEmptyModelPatch(patch)).toBe(true);
	});

	it('sends a changed token limit as a number', () => {
		const patch = buildModelPatch(model, { ...modelForm(model), maxOutput: '16 384' });
		expect(patch.max_output_tokens).toBe(16384);
		expect(patch.max_input_tokens).toBeNull();
	});

	it('never sends the context window, known or unknown, so the stored value is kept', () => {
		const known = buildModelPatch(model, { ...modelForm(model), maxOutput: '4096' });
		expect(known.context_window_tokens).toBeNull();
		expect(known.clear_context_window).toBe(false);
		const unknown = { ...model, context_window_tokens: null };
		const patch = buildModelPatch(unknown, { ...modelForm(unknown), name: 'Renamed' });
		expect(patch.context_window_tokens).toBeNull();
		expect(patch.clear_context_window).toBe(false);
	});

	it('does not send modalities reordered or unchanged', () => {
		const reordered = { ...modelForm(model), modalities: ['image', 'text'] };
		expect(buildModelPatch(model, reordered).input_modalities).toBeNull();
	});

	it('sends the modality list when it changed, in display order', () => {
		const form = { ...modelForm(model), modalities: ['pdf', 'text'] };
		expect(buildModelPatch(model, form).input_modalities).toEqual(['text', 'pdf']);
	});

	it('keeps modalities the form does not offer', () => {
		const exotic = { ...model, input_modalities: ['text', 'hologram'] };
		const form = { ...modelForm(exotic), modalities: ['text', 'hologram', 'image'] };
		expect(buildModelPatch(exotic, form).input_modalities).toEqual(['text', 'image', 'hologram']);
	});

	it('changes the default profile, or clears it', () => {
		expect(buildModelPatch(model, { ...modelForm(model), reasoningDefault: 'high' })).toMatchObject({
			reasoning_default: 'high',
			clear_reasoning_default: false
		});
		expect(buildModelPatch(model, { ...modelForm(model), reasoningDefault: null })).toMatchObject({
			reasoning_default: null,
			clear_reasoning_default: true
		});
	});

	it('does not clear a default profile that was already absent', () => {
		const none = { ...model, reasoning_default: null };
		const patch = buildModelPatch(none, modelForm(none));
		expect(patch.clear_reasoning_default).toBe(false);
		expect(patch.reasoning_default).toBeNull();
	});
});
