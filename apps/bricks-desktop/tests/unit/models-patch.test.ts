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
	api_key: 'sk-test-saved',
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
	async_tool_calls: false,
	max_input_tokens: 128000,
	max_output_tokens: 8192,
	profiles: [
		{ id: 'low', label: 'Léger', parameters: {}, remove: [] },
		{ id: 'high', label: 'Approfondi', parameters: {}, remove: [] }
	],
	priced: true,
	document_mime_types: [],
	pricing: { input: '0.30', output: '1.20', cache_read: '0.03' }
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
			api_key: null
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

	it('sends the key only when it changed, and an empty one to remove it', () => {
		expect(providerForm(provider).apiKey).toBe('sk-test-saved');
		const form = { ...providerForm(provider), apiKey: 'sk-test-placeholder' };
		expect(buildProviderPatch(provider, form).api_key).toBe('sk-test-placeholder');
		expect(buildProviderPatch(provider, { ...form, apiKey: '' }).api_key).toBe('');
		expect(buildProviderPatch(provider, { ...form, apiKey: ' sk-test-saved ' }).api_key).toBeNull();
	});
});

describe('buildModelPatch', () => {
	it('sends nothing when the form is untouched', () => {
		const patch = buildModelPatch(model, modelForm(model));
		expect(isEmptyModelPatch(patch)).toBe(true);
	});

	it('sends asynchronous tool calls when the box changes, off included', () => {
		const on = buildModelPatch(model, { ...modelForm(model), asyncToolCalls: true });
		expect(on.async_tool_calls).toBe(true);
		const enabled = { ...model, async_tool_calls: true };
		const off = buildModelPatch(enabled, { ...modelForm(enabled), asyncToolCalls: false });
		expect(off.async_tool_calls).toBe(false);
		expect(isEmptyModelPatch(off)).toBe(false);
		expect(isEmptyModelPatch(buildModelPatch(enabled, modelForm(enabled)))).toBe(true);
	});

	it('sends a changed token limit as a number', () => {
		const patch = buildModelPatch(model, { ...modelForm(model), maxOutput: '16 384' });
		expect(patch.max_output_tokens).toBe(16384);
		expect(patch.max_input_tokens).toBeNull();
	});

	it('never sends a context window: only the input and output limits exist', () => {
		const patch = buildModelPatch(model, { ...modelForm(model), maxOutput: '4096' });
		expect('context_window_tokens' in patch).toBe(false);
		expect('clear_context_window' in patch).toBe(false);
	});

	it('never sends a default profile: the first level applies when none is chosen', () => {
		const patch = buildModelPatch(model, { ...modelForm(model), asyncToolCalls: true });
		expect(patch).not.toHaveProperty('reasoning_default');
		expect(patch).not.toHaveProperty('clear_reasoning_default');
	});

	it('does not send modalities reordered or unchanged', () => {
		const reordered = { ...modelForm(model), modalities: ['image', 'text'] };
		expect(buildModelPatch(model, reordered).input_modalities).toBeNull();
	});

	it('sends the modality list when it changed, in display order', () => {
		const form = { ...modelForm(model), modalities: ['document', 'image', 'text'] };
		expect(buildModelPatch(model, form).input_modalities).toEqual(['text', 'image', 'document']);
	});

	it('keeps modalities the form does not offer', () => {
		const exotic = { ...model, input_modalities: ['text', 'hologram'] };
		const form = { ...modelForm(exotic), modalities: ['text', 'hologram', 'image'] };
		expect(buildModelPatch(exotic, form).input_modalities).toEqual(['text', 'image', 'hologram']);
	});
});
