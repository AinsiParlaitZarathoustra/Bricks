// Unit tests for the required-field rules of the models settings: no empty string reaches the engine
// for a required field, optional fields are cleared only when the user cleared them, and the key
// variable is never sent empty.
import { describe, expect, it } from 'vitest';
import type { ModelView } from '../../src/lib/ipc/bindings/ModelView';
import type { ProviderView } from '../../src/lib/ipc/bindings/ProviderView';
import {
	API_KEY_ENV_CLEAR_MESSAGE,
	API_KEY_ENV_REQUIRED_MESSAGE,
	REQUIRED_MESSAGE,
	buildModelPatch,
	buildProviderPatch,
	hasFormErrors,
	modelForm,
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
	input_modalities: ['text'],
	output_modalities: ['text'],
	streaming: true,
	async_tool_calls: false,
	max_input_tokens: 128000,
	max_output_tokens: 8192,
	reasoning_default: null,
	profiles: [],
	priced: false,
	document_mime_types: [],
	pricing: null
};

describe('model required texts', () => {
	it('refuses an empty name or model API name', () => {
		const errors = validateModelForm({ ...modelForm(model), name: '  ', apiModel: '' });
		expect(errors.name).toBe(REQUIRED_MESSAGE);
		expect(errors.apiModel).toBe(REQUIRED_MESSAGE);
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
		const errors = validateProviderForm(form, provider);
		expect(errors.name).toBe(REQUIRED_MESSAGE);
		expect(errors.endpoint).toBe(REQUIRED_MESSAGE);
		const patch = buildProviderPatch(provider, form);
		expect(patch.name).toBeNull();
		expect(patch.endpoint).toBeNull();
	});
});

describe('api_key_env', () => {
	it('requires a variable name while the authentication needs a key', () => {
		const form = { ...providerForm(provider), apiKeyEnv: '' };
		expect(validateProviderForm(form, provider).apiKeyEnv).toBe(API_KEY_ENV_REQUIRED_MESSAGE);
	});

	it('does not require one when the authentication is none and none was set', () => {
		const none = { ...provider, auth: 'none', api_key_env: null };
		const form = { ...providerForm(none), apiKeyEnv: '' };
		expect(validateProviderForm(form, none).apiKeyEnv).toBeUndefined();
	});

	it('does not require one on a provider with an inline key (the field is disabled there)', () => {
		const inline = { ...provider, has_inline_key: true, api_key_env: null };
		const form = { ...providerForm(inline), apiKeyEnv: '' };
		expect(validateProviderForm(form, inline).apiKeyEnv).toBeUndefined();
	});

	it('refuses to clear a variable that cannot be removed in this window', () => {
		const none = { ...provider, auth: 'none' };
		const form = { ...providerForm(none), apiKeyEnv: '' };
		expect(validateProviderForm(form, none).apiKeyEnv).toBe(API_KEY_ENV_CLEAR_MESSAGE);
	});

	it('is never sent as an empty string', () => {
		const cleared = { ...providerForm(provider), apiKeyEnv: '' };
		expect(buildProviderPatch(provider, cleared).api_key_env).toBeNull();
		const none = { ...provider, auth: 'none' };
		expect(buildProviderPatch(none, { ...providerForm(none), apiKeyEnv: '' }).api_key_env).toBeNull();
	});

	it('is sent when it changed to a name', () => {
		const form = { ...providerForm(provider), apiKeyEnv: 'OTHER_KEY' };
		expect(buildProviderPatch(provider, form).api_key_env).toBe('OTHER_KEY');
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
		expect(hasFormErrors(validateProviderForm(form, provider))).toBe(false);
		const patch = buildProviderPatch(provider, form);
		for (const value of Object.values(patch)) expect(value).not.toBe('');
	});
});
