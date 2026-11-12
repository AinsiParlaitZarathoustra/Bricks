// Unit tests for the required-field rules of the models settings: no empty string reaches the engine
// for a required field, optional fields are cleared only when the user cleared them, and the API key
// is asked for a new provider only when its authentication needs one, and never sent in the patch.
import { describe, expect, it } from 'vitest';
import type { ModelView } from '../../src/lib/ipc/bindings/ModelView';
import type { ProviderView } from '../../src/lib/ipc/bindings/ProviderView';
import {
	REQUIRED_MESSAGE,
	buildModelPatch,
	buildProviderPatch,
	hasFormErrors,
	isEmptyProviderPatch,
	modelForm,
	newProviderForm,
	providerForm,
	validateModelForm,
	validateProviderForm
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
	input_modalities: ['text'],
	output_modalities: ['text'],
	streaming: true,
	async_tool_calls: false,
	max_input_tokens: 128000,
	max_output_tokens: 8192,
	profiles: [],
	priced: false,
	document_mime_types: [],
	pricing: null
};

describe('model required texts', () => {
	it('refuses an empty name or model API name', () => {
		const errors = validateModelForm({ ...modelForm(model), name: '  ', apiModel: '' });
		expect(errors.name).toBe(REQUIRED_MESSAGE());
		expect(errors.apiModel).toBe(REQUIRED_MESSAGE());
		expect(hasFormErrors(errors)).toBe(true);
	});

	it('never sends an empty name or model API name', () => {
		const patch = buildModelPatch(model, { ...modelForm(model), name: '  ', apiModel: '' });
		expect(patch.name).toBeNull();
		expect(patch.api_model).toBeNull();
	});
});

describe('provider required texts', () => {
	it('refuses an empty name or endpoint, and never sends them empty', () => {
		const form = { ...providerForm(provider), name: ' ', endpoint: '' };
		const errors = validateProviderForm(form);
		expect(errors.name).toBe(REQUIRED_MESSAGE());
		expect(errors.endpoint).toBe(REQUIRED_MESSAGE());
		const patch = buildProviderPatch(provider, form);
		expect(patch.name).toBeNull();
		expect(patch.endpoint).toBeNull();
	});
});

describe('API key of a provider', () => {
	const fresh = { ...newProviderForm(), name: 'Acme', endpoint: 'https://api.acme.test/v1' };

	it('requires a key for a new provider whose authentication needs one, blank text included', () => {
		expect(validateProviderForm(fresh).apiKey).toBe(REQUIRED_MESSAGE());
		expect(validateProviderForm({ ...fresh, apiKey: '   ' }).apiKey).toBe(REQUIRED_MESSAGE());
	});

	it('accepts a pasted key for a new provider', () => {
		expect(validateProviderForm({ ...fresh, apiKey: 'sk-test-placeholder' }).apiKey).toBeUndefined();
	});

	it('does not require a key for a new provider with no authentication', () => {
		expect(validateProviderForm({ ...fresh, auth: 'none' }).apiKey).toBeUndefined();
	});

	it('requires the key when editing too: emptying it is refused', () => {
		expect(validateProviderForm({ ...providerForm(provider), apiKey: '' }).apiKey).toBe(REQUIRED_MESSAGE());
		expect(validateProviderForm(providerForm(provider)).apiKey).toBeUndefined();
	});

	it('puts a changed key in the provider patch', () => {
		const form = { ...providerForm(provider), apiKey: 'sk-test-placeholder' };
		const patch = buildProviderPatch(provider, form);
		expect(patch.api_key).toBe('sk-test-placeholder');
		expect(isEmptyProviderPatch(patch)).toBe(false);
	});
});

describe('optional auth header', () => {
	it('is cleared with an empty string when the user emptied a value that was set', () => {
		const withHeader = { ...provider, auth: 'api_key_header', auth_header: 'X-Key' };
		const form = { ...providerForm(withHeader), authHeader: '' };
		expect(buildProviderPatch(withHeader, form).auth_header).toBe('');
	});

	it('stays unchanged when it was empty and is still empty', () => {
		const form = { ...providerForm(provider), authHeader: '' };
		expect(buildProviderPatch(provider, form).auth_header).toBeNull();
	});
});

describe('valid forms', () => {
	it('report no error and no empty string for a provider with nothing to change', () => {
		const form = providerForm(provider);
		expect(hasFormErrors(validateProviderForm(form))).toBe(false);
		const patch = buildProviderPatch(provider, form);
		for (const value of Object.values(patch)) expect(value).not.toBe('');
	});
});
