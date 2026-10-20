// Unit tests for creating providers and models, and for the prices and accepted document types of
// the model form: parsing, validation, the full create patches, the edit patch for prices and
// document types, and the count of unpriced models.
import { describe, expect, it } from 'vitest';
import type { ModelView } from '../../src/lib/ipc/bindings/ModelView';
import type { ProviderView } from '../../src/lib/ipc/bindings/ProviderView';
import { formatPrice } from '../../src/lib/features/settings/format';
import {
	DOCUMENT_TYPES_REQUIRED,
	DOCUMENT_TYPE_INVALID,
	ID_DUPLICATE,
	ID_ERROR,
	PRICE_ERROR,
	REQUIRED_MESSAGE,
	buildModelCreatePatch,
	buildModelPatch,
	buildProviderCreatePatch,
	documentTypes,
	hasFormErrors,
	isEmptyModelPatch,
	modelForm,
	newModelForm,
	newProviderForm,
	parsePrice,
	unpricedModels,
	validateModelForm,
	validateNewId,
	type ModelForm
} from '../../src/lib/features/settings/models';

const model: ModelView = {
	id: 'fast',
	selection: 'acme/fast',
	name: 'Fast',
	api_model: 'acme-fast-1',
	protocol: null,
	endpoint: null,
	input_modalities: ['text', 'document'],
	output_modalities: ['text'],
	streaming: true,
	tool_calls: true,
	max_input_tokens: 128000,
	max_output_tokens: 8192,
	context_window_tokens: null,
	reasoning_default: null,
	profiles: [],
	priced: true,
	document_mime_types: ['application/pdf'],
	pricing: { input: '0.30', output: '1.20', cache_read: '0.03' }
};

/** A form that passes validation, as a new model would be typed. */
function filledForm(overrides: Partial<ModelForm> = {}): ModelForm {
	return {
		...newModelForm(),
		name: 'Fast',
		apiModel: 'acme-fast-1',
		maxInput: '128000',
		maxOutput: '8192',
		priceInput: '0.30',
		priceOutput: '1.20',
		priceCache: '0.03',
		...overrides
	};
}

function providerWith(models: ModelView[]): ProviderView {
	return {
		id: 'acme',
		name: 'Acme',
		endpoint: 'https://api.acme.test/v1',
		protocol: 'chat_completions',
		auth: 'none',
		auth_header: null,
		api_key_env: null,
		has_inline_key: false,
		has_credentials: false,
		path: null,
		header_names: [],
		models
	};
}

function modelWithPricing(pricing: ModelView['pricing']): ModelView {
	return { ...model, pricing, priced: pricing !== null };
}

describe('parsePrice', () => {
	it('accepts zero and decimals, with a comma as decimal point', () => {
		expect(parsePrice('0')).toBe('0');
		expect(parsePrice('0.30')).toBe('0.30');
		expect(parsePrice('1,5')).toBe('1.5');
		expect(parsePrice(' 12 ')).toBe('12');
	});

	it('refuses negatives, text and empty values', () => {
		expect(parsePrice('-1')).toBeNull();
		expect(parsePrice('abc')).toBeNull();
		expect(parsePrice('')).toBeNull();
	});
});

describe('documentTypes', () => {
	it('splits on commas, semicolons and spaces, keeping the order', () => {
		expect(documentTypes('application/pdf, image/png;text/plain  */*')).toEqual([
			'application/pdf',
			'image/png',
			'text/plain',
			'*/*'
		]);
	});

	it('drops repeats and empty pieces', () => {
		expect(documentTypes('application/pdf, application/pdf ,, ')).toEqual(['application/pdf']);
		expect(documentTypes('')).toEqual([]);
	});
});

describe('validateNewId', () => {
	const taken = ['acme', 'fast'];

	it('refuses an empty id', () => {
		expect(validateNewId('', taken)).toBe(REQUIRED_MESSAGE);
		expect(validateNewId('   ', taken)).toBe(REQUIRED_MESSAGE);
	});

	it('refuses characters outside letters, digits, dot, underscore and dash', () => {
		expect(validateNewId('mon modèle', taken)).toBe(ID_ERROR);
		expect(validateNewId('a/b', taken)).toBe(ID_ERROR);
	});

	it('refuses an id that starts with a dash or a dot', () => {
		expect(validateNewId('-fast', taken)).toBe(ID_ERROR);
		expect(validateNewId('.fast', taken)).toBe(ID_ERROR);
	});

	it('refuses an id already taken', () => {
		expect(validateNewId('fast', taken)).toBe(ID_DUPLICATE);
		expect(validateNewId(' acme ', taken)).toBe(ID_DUPLICATE);
	});

	it('accepts a free id', () => {
		expect(validateNewId('fast-2.v1_beta', taken)).toBeUndefined();
		expect(validateNewId('2fast', taken)).toBeUndefined();
	});
});

describe('validateModelForm for a new model', () => {
	it('requires the three prices', () => {
		const errors = validateModelForm(filledForm({ priceInput: '', priceOutput: '', priceCache: '' }));
		expect(errors.priceInput).toBe(PRICE_ERROR);
		expect(errors.priceOutput).toBe(PRICE_ERROR);
		expect(errors.priceCache).toBe(PRICE_ERROR);
	});

	it('refuses a negative or text price', () => {
		expect(validateModelForm(filledForm({ priceInput: '-1' })).priceInput).toBe(PRICE_ERROR);
		expect(validateModelForm(filledForm({ priceOutput: 'gratuit' })).priceOutput).toBe(PRICE_ERROR);
	});

	it('accepts a free model priced at zero', () => {
		expect(hasFormErrors(validateModelForm(filledForm({ priceInput: '0', priceOutput: '0', priceCache: '0' })))).toBe(
			false
		);
	});

	it('requires document types when the document input is checked', () => {
		const errors = validateModelForm(filledForm({ modalities: ['text', 'document'], documentTypes: ' ' }));
		expect(errors.documentTypes).toBe(DOCUMENT_TYPES_REQUIRED);
	});

	it('refuses a type that is not type/subtype, such as pdf', () => {
		const errors = validateModelForm(filledForm({ modalities: ['document'], documentTypes: 'pdf' }));
		expect(errors.documentTypes).toBe(DOCUMENT_TYPE_INVALID);
	});

	it('accepts application/pdf and */* as document types', () => {
		expect(
			validateModelForm(filledForm({ modalities: ['document'], documentTypes: 'application/pdf' })).documentTypes
		).toBeUndefined();
		expect(validateModelForm(filledForm({ modalities: ['document'], documentTypes: '*/*' })).documentTypes).toBeUndefined();
	});

	it('does not check document types when the document input is not checked', () => {
		expect(validateModelForm(filledForm({ modalities: ['text'], documentTypes: 'pdf' })).documentTypes).toBeUndefined();
	});
});

describe('buildModelCreatePatch', () => {
	it('sends every field, with the prices as an object and the modalities in display order', () => {
		const patch = buildModelCreatePatch(
			filledForm({
				name: '  Fast  ',
				apiModel: ' acme-fast-1 ',
				maxInput: '128 000',
				modalities: ['document', 'text', 'image'],
				documentTypes: 'application/pdf',
				priceInput: '0,30',
				priceOutput: '1.20',
				priceCache: '0'
			})
		);
		expect(patch).toEqual({
			name: 'Fast',
			api_model: 'acme-fast-1',
			max_input_tokens: 128000,
			max_output_tokens: 8192,
			context_window_tokens: null,
			clear_context_window: false,
			input_modalities: ['text', 'image', 'document'],
			document_mime_types: ['application/pdf'],
			pricing: { input: '0.30', output: '1.20', cache_read: '0' },
			reasoning_default: null,
			clear_reasoning_default: false,
			profiles: null
		});
	});

	it('sends no document types when the document input is not checked', () => {
		const patch = buildModelCreatePatch(
			filledForm({ modalities: ['text', 'image'], documentTypes: 'application/pdf' })
		);
		expect(patch.document_mime_types).toBeNull();
		expect(patch.input_modalities).toEqual(['text', 'image']);
	});

	it('sends the profiles only when there are some', () => {
		const profile = { id: 'low', label: null, parameters: {}, remove: [] };
		expect(buildModelCreatePatch(filledForm(), [profile]).profiles).toEqual([profile]);
		expect(buildModelCreatePatch(filledForm(), []).profiles).toBeNull();
	});
});

describe('buildModelPatch for prices and document types', () => {
	it('sends nothing for prices when none changed', () => {
		const patch = buildModelPatch(model, modelForm(model));
		expect(patch.pricing).toBeNull();
		expect(isEmptyModelPatch(patch)).toBe(true);
	});

	it('sends the three prices when one of them changed', () => {
		const patch = buildModelPatch(model, { ...modelForm(model), priceInput: '0.31' });
		expect(patch.pricing).toEqual({ input: '0.31', output: '1.20', cache_read: '0.03' });
	});

	it('does not send a price typed with the same value', () => {
		const patch = buildModelPatch(model, { ...modelForm(model), priceInput: '0.3', priceOutput: '1,20' });
		expect(patch.pricing).toBeNull();
	});

	it('sends the prices when the model had none', () => {
		const unpriced = modelWithPricing(null);
		const patch = buildModelPatch(unpriced, modelForm(unpriced));
		expect(patch.pricing).toBeNull();
		const filled = buildModelPatch(unpriced, { ...modelForm(unpriced), priceInput: '0', priceOutput: '0', priceCache: '0' });
		expect(filled.pricing).toEqual({ input: '0', output: '0', cache_read: '0' });
	});

	it('sends an empty list of document types when the document input is unchecked', () => {
		const patch = buildModelPatch(model, { ...modelForm(model), modalities: ['text'] });
		expect(patch.document_mime_types).toEqual([]);
		expect(patch.input_modalities).toEqual(['text']);
	});

	it('sends nothing for document types when they did not change', () => {
		expect(buildModelPatch(model, modelForm(model)).document_mime_types).toBeNull();
	});

	it('sends the new list when the types change', () => {
		const patch = buildModelPatch(model, { ...modelForm(model), documentTypes: 'application/pdf, */*' });
		expect(patch.document_mime_types).toEqual(['application/pdf', '*/*']);
	});

	it('sends nothing for document types of a model with none, the input being unchecked', () => {
		const text = { ...model, input_modalities: ['text'], document_mime_types: [] };
		expect(buildModelPatch(text, modelForm(text)).document_mime_types).toBeNull();
	});
});

describe('buildProviderCreatePatch', () => {
	it('trims the texts and sends null for an empty header and an empty variable', () => {
		const patch = buildProviderCreatePatch({
			...newProviderForm(),
			name: '  Acme ',
			endpoint: ' https://api.acme.test/v1 ',
			authHeader: '   ',
			apiKeyEnv: ''
		});
		expect(patch).toEqual({
			name: 'Acme',
			endpoint: 'https://api.acme.test/v1',
			protocol: 'chat_completions',
			auth: 'bearer',
			auth_header: null,
			api_key_env: null
		});
	});

	it('sends the header and the variable when they are typed', () => {
		const patch = buildProviderCreatePatch({
			...newProviderForm(),
			name: 'Acme',
			endpoint: 'https://api.acme.test/v1',
			auth: 'api_key_header',
			authHeader: ' X-Key ',
			apiKeyEnv: ' ACME_KEY '
		});
		expect(patch).toMatchObject({ auth: 'api_key_header', auth_header: 'X-Key', api_key_env: 'ACME_KEY' });
	});
});

describe('unpricedModels', () => {
	it('counts a model with no pricing at all', () => {
		expect(unpricedModels([providerWith([modelWithPricing(null)])])).toBe(1);
	});

	it('counts a model with one of the three prices missing', () => {
		const partial = modelWithPricing({ input: '0.3', output: null, cache_read: '0' });
		const noCache = modelWithPricing({ input: '0.3', output: '1', cache_read: null });
		expect(unpricedModels([providerWith([partial, noCache])])).toBe(2);
	});

	it('does not count a fully priced model, free models included', () => {
		const free = modelWithPricing({ input: '0', output: '0', cache_read: '0' });
		expect(unpricedModels([providerWith([model, free])])).toBe(0);
	});

	it('adds up over providers', () => {
		const providers = [providerWith([modelWithPricing(null)]), providerWith([]), providerWith([modelWithPricing(null)])];
		expect(unpricedModels(providers)).toBe(2);
		expect(unpricedModels([])).toBe(0);
	});
});

describe('formatPrice', () => {
	it('writes a price per million tokens in French, with the dollar sign', () => {
		expect(formatPrice('0.3')).toBe('0,30 $');
		expect(formatPrice('0')).toBe('0,00 $');
	});

	it('shows a dash when the price is not in the file', () => {
		expect(formatPrice(null)).toBe('—');
		expect(formatPrice(undefined)).toBe('—');
	});
});
