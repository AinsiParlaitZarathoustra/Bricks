// Pure helpers of the « Modèles » settings: form state, client validation,
// patch building, labels and write-failure mapping. No state, no IPC: the
// write calls live in ModelSettings.svelte. Rust validates the rest.

import type { ModelPatch } from '$lib/ipc/bindings/ModelPatch';
import type { ModelProfileView } from '$lib/ipc/bindings/ModelProfileView';
import type { ModelView } from '$lib/ipc/bindings/ModelView';
import type { PricingEdit } from '$lib/ipc/bindings/PricingEdit';
import type { ProviderPatch } from '$lib/ipc/bindings/ProviderPatch';
import type { ProviderView } from '$lib/ipc/bindings/ProviderView';
import type { ReasoningProfileEdit } from '$lib/ipc/bindings/ReasoningProfileEdit';
import { formatCount } from './format';

export const PROTOCOL_OPTIONS: readonly { value: string; label: string }[] = [
	{ value: 'chat_completions', label: 'Chat Completions' },
	{ value: 'responses', label: 'Responses' },
	{ value: 'anthropic_messages', label: 'Anthropic Messages' }
];

export const AUTH_OPTIONS: readonly { value: string; label: string }[] = [
	{ value: 'bearer', label: 'Jeton porteur (Bearer)' },
	{ value: 'api_key_header', label: 'Clé dans un en-tête' },
	{ value: 'none', label: 'Aucune' }
];

/** Input modalities the form offers, in display order (unknown ones are kept as they are). */
export const MODALITY_OPTIONS: readonly string[] = ['text', 'image', 'audio', 'video', 'document'];

/** Accepted file types offered with the document input. */
export const DOCUMENT_TYPE_PRESETS: readonly { value: string; label: string }[] = [
	{ value: 'application/pdf', label: 'PDF' },
	{ value: '*/*', label: 'Tout fichier' }
];

export const CONFLICT_MESSAGE =
	'Le fichier des fournisseurs a été modifié ailleurs. Vos modifications n’ont pas été enregistrées : les valeurs ont été rechargées, vérifiez-les avant d’enregistrer à nouveau.';

export const COUNT_ERROR = 'Saisissez un nombre entier supérieur à 0.';
export const PRICE_ERROR = 'Saisissez un nombre positif ou nul, par exemple 0.30.';
export const ID_ERROR = 'Lettres, chiffres, « . », « _ » et « - », en commençant par une lettre ou un chiffre.';
export const ID_DUPLICATE = 'Cet identifiant existe déjà.';
export const MODALITY_REQUIRED = 'Cochez au moins une entrée.';
export const DOCUMENT_TYPES_REQUIRED = 'Indiquez au moins un type de fichier.';
export const DOCUMENT_TYPE_INVALID = 'Chaque type s’écrit type/sous-type, par exemple application/pdf.';
export const REQUIRED_MESSAGE = 'Ce champ est requis.';
export const API_KEY_ENV_REQUIRED_MESSAGE =
	'Indiquez la variable qui contient la clé : elle est requise avec cette authentification.';
export const API_KEY_ENV_CLEAR_MESSAGE = 'Cette variable ne peut pas être effacée depuis cette fenêtre.';

export const PROFILE_ID_REQUIRED = 'Saisissez un identifiant.';
export const PROFILE_ID_DUPLICATE = 'Cet identifiant existe déjà pour ce modèle.';
export const PROFILE_PARAMETERS_OBJECT = 'Les paramètres doivent être un objet JSON, entre accolades.';
export const PROFILE_POINTER = 'Chaque chemin commence par « / », par exemple /champ/sous-champ.';
export const DEFAULT_PROFILE_PENDING =
	'Le profil par défaut a été retiré ou renommé : choisissez-en un autre, ou « Aucun ».';

/** The name to show: the name when it has text, else the id (an empty label would render blank). */
export function displayName(name: string | null | undefined, id: string): string {
	const text = name?.trim() ?? '';
	return text === '' ? id : text;
}

/** Label of an effort profile, with its id as the fallback. */
export function profileLabel(profile: ModelProfileView): string {
	return displayName(profile.label, profile.id);
}

export function protocolLabel(value: string): string {
	return PROTOCOL_OPTIONS.find((o) => o.value === value)?.label ?? value;
}

export function authLabel(value: string): string {
	return AUTH_OPTIONS.find((o) => o.value === value)?.label ?? value;
}

/** « 1 modèle », « 3 modèles » (digits grouped as in the rest of the dialog). */
export function modelCountLabel(count: number): string {
	return `${formatCount(count)} ${count === 1 ? 'modèle' : 'modèles'}`;
}

function isJsonObject(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}

// ─── Forms ───────────────────────────────────────────────────────────────────

export interface ProviderForm {
	name: string;
	endpoint: string;
	protocol: string;
	auth: string;
	authHeader: string;
	apiKeyEnv: string;
}

export function newProviderForm(): ProviderForm {
	return { name: '', endpoint: '', protocol: 'chat_completions', auth: 'bearer', authHeader: '', apiKeyEnv: '' };
}

export function providerForm(provider: ProviderView): ProviderForm {
	return {
		name: provider.name,
		endpoint: provider.endpoint,
		protocol: provider.protocol,
		auth: provider.auth,
		authHeader: provider.auth_header ?? '',
		apiKeyEnv: provider.api_key_env ?? ''
	};
}

/**
 * An effort profile as the form edits it. The JSON texts stay as typed until they are checked, so
 * a mistake is shown and nothing is written.
 */
export interface ProfileDraft {
	id: string;
	label: string;
	/** JSON text of the `parameters` object. */
	parameters: string;
	/** One JSON Pointer per line. */
	remove: string;
}

export function profileDraft(profile: ModelProfileView): ProfileDraft {
	return {
		id: profile.id,
		label: profile.label ?? '',
		parameters: JSON.stringify(profile.parameters, null, 2),
		remove: profile.remove.join('\n')
	};
}

export function newProfileDraft(): ProfileDraft {
	return { id: '', label: '', parameters: '{}', remove: '' };
}

/** Label of a draft in the form: its label when it has text, else its id. */
export function draftLabel(draft: ProfileDraft): string {
	return displayName(draft.label, draft.id.trim());
}

/** The pointers typed: one per line, blank lines ignored, surrounding spaces removed. */
export function pointerLines(text: string): string[] {
	return text
		.split('\n')
		.map((line) => line.trim())
		.filter((line) => line !== '');
}

export interface ProfileErrors {
	id?: string;
	parameters?: string;
	remove?: string;
}

export type ProfileParse = { ok: true; value: ReasoningProfileEdit } | { ok: false; errors: ProfileErrors };

/**
 * Checks one profile as typed and turns it into the engine's shape. `takenIds` are the ids of the
 * other profiles of the model. These are the client rules only: the engine refuses protected
 * fields and the rest, and its refusal is shown as it is.
 */
export function parseProfileDraft(draft: ProfileDraft, takenIds: readonly string[]): ProfileParse {
	const errors: ProfileErrors = {};
	const id = draft.id.trim();
	if (id === '') errors.id = PROFILE_ID_REQUIRED;
	else if (takenIds.includes(id)) errors.id = PROFILE_ID_DUPLICATE;

	let parameters: Record<string, unknown> | null = null;
	try {
		const parsed: unknown = JSON.parse(draft.parameters.trim() === '' ? '{}' : draft.parameters);
		if (isJsonObject(parsed)) parameters = parsed;
		else errors.parameters = PROFILE_PARAMETERS_OBJECT;
	} catch (e) {
		errors.parameters = `JSON invalide : ${e instanceof Error ? e.message : String(e)}`;
	}

	const remove = pointerLines(draft.remove);
	if (remove.some((pointer) => !pointer.startsWith('/'))) errors.remove = PROFILE_POINTER;

	if (errors.id || errors.parameters || errors.remove || parameters === null) return { ok: false, errors };
	const label = draft.label.trim();
	return { ok: true, value: { id, label: label === '' ? null : label, parameters, remove } };
}

export interface ModelForm {
	name: string;
	apiModel: string;
	maxInput: string;
	maxOutput: string;
	modalities: string[];
	/** Accepted document types, separated by commas or spaces. */
	documentTypes: string;
	/** USD per million tokens, as typed. */
	priceInput: string;
	priceOutput: string;
	priceCache: string;
	profiles: ProfileDraft[];
	/** Default profile id, `null` for none. */
	reasoningDefault: string | null;
	/**
	 * The default profile was removed or renamed: the user must choose again. `null` (none) is a
	 * valid choice; leaving the choice open is not.
	 */
	defaultPending: boolean;
}

export function modelForm(model: ModelView): ModelForm {
	return {
		name: model.name,
		apiModel: model.api_model,
		maxInput: String(model.max_input_tokens),
		maxOutput: String(model.max_output_tokens),
		modalities: [...model.input_modalities],
		documentTypes: model.document_mime_types.join(', '),
		priceInput: model.pricing?.input ?? '',
		priceOutput: model.pricing?.output ?? '',
		priceCache: model.pricing?.cache_read ?? '',
		profiles: model.profiles.map(profileDraft),
		reasoningDefault: model.reasoning_default,
		defaultPending: false
	};
}

export function newModelForm(): ModelForm {
	return {
		name: '',
		apiModel: '',
		maxInput: '',
		maxOutput: '',
		modalities: ['text'],
		documentTypes: '',
		priceInput: '',
		priceOutput: '',
		priceCache: '',
		profiles: [],
		reasoningDefault: null,
		defaultPending: false
	};
}

// ─── Validation ──────────────────────────────────────────────────────────────

/** A whole number above zero typed by the user (spaces and underscores allowed), or `null`. */
export function parseCount(text: string): number | null {
	const digits = text.replace(/[\s_  ]/g, '');
	if (!/^\d+$/.test(digits)) return null;
	const value = Number(digits);
	return Number.isSafeInteger(value) && value > 0 ? value : null;
}

/** A price as typed: a decimal number, 0 allowed (spaces ignored, comma as decimal point accepted). */
export function parsePrice(text: string): string | null {
	const value = text.replace(/[\s  ]/g, '').replace(',', '.');
	return /^\d+(\.\d+)?$/.test(value) ? value : null;
}

/** The document types typed, in order, without repeats. */
export function documentTypes(text: string): string[] {
	return [...new Set(text.split(/[\s,;]+/).filter((t) => t !== ''))];
}

const MIME = /^[^\s/]+\/[^\s/]+$/;

/** A new id: letters, digits, `.`, `_` and `-`, starting with a letter or a digit. */
export function validateNewId(id: string, taken: readonly string[]): string | undefined {
	const value = id.trim();
	if (value === '') return REQUIRED_MESSAGE;
	if (!/^[A-Za-z0-9][A-Za-z0-9._-]*$/.test(value)) return ID_ERROR;
	if (taken.includes(value)) return ID_DUPLICATE;
	return undefined;
}

export interface ModelFormErrors {
	name?: string;
	modalities?: string;
	documentTypes?: string;
	priceInput?: string;
	priceOutput?: string;
	priceCache?: string;
	apiModel?: string;
	maxInput?: string;
	maxOutput?: string;
	defaultProfile?: string;
}

export interface ProviderFormErrors {
	name?: string;
	endpoint?: string;
	apiKeyEnv?: string;
}

/**
 * Client checks: required texts are filled, token counts are whole numbers above zero, and the
 * default profile is one of the profiles (or none). Rust checks the rest.
 */
export function validateModelForm(form: ModelForm): ModelFormErrors {
	const errors: ModelFormErrors = {};
	if (form.name.trim() === '') errors.name = REQUIRED_MESSAGE;
	if (form.apiModel.trim() === '') errors.apiModel = REQUIRED_MESSAGE;
	if (parseCount(form.maxInput) === null) errors.maxInput = COUNT_ERROR;
	if (parseCount(form.maxOutput) === null) errors.maxOutput = COUNT_ERROR;
	if (form.modalities.length === 0) errors.modalities = MODALITY_REQUIRED;
	if (form.modalities.includes('document')) {
		const types = documentTypes(form.documentTypes);
		if (types.length === 0) errors.documentTypes = DOCUMENT_TYPES_REQUIRED;
		else if (!types.every((t) => MIME.test(t))) errors.documentTypes = DOCUMENT_TYPE_INVALID;
	}
	if (parsePrice(form.priceInput) === null) errors.priceInput = PRICE_ERROR;
	if (parsePrice(form.priceOutput) === null) errors.priceOutput = PRICE_ERROR;
	if (parsePrice(form.priceCache) === null) errors.priceCache = PRICE_ERROR;
	const ids = form.profiles.map((p) => p.id.trim());
	if (form.defaultPending || (form.reasoningDefault !== null && !ids.includes(form.reasoningDefault))) {
		errors.defaultProfile = DEFAULT_PROFILE_PENDING;
	}
	return errors;
}

/** `provider` is `null` for a new provider. */
export function validateProviderForm(form: ProviderForm, provider: ProviderView | null): ProviderFormErrors {
	const errors: ProviderFormErrors = {};
	if (form.name.trim() === '') errors.name = REQUIRED_MESSAGE;
	if (form.endpoint.trim() === '') errors.endpoint = REQUIRED_MESSAGE;
	// A provider with an inline key has no variable to name: the field is disabled there.
	if (provider === null) {
		if (form.apiKeyEnv.trim() === '' && form.auth !== 'none') errors.apiKeyEnv = API_KEY_ENV_REQUIRED_MESSAGE;
	} else if (!provider.has_inline_key) {
		const variable = form.apiKeyEnv.trim();
		if (variable === '' && form.auth !== 'none') {
			errors.apiKeyEnv = API_KEY_ENV_REQUIRED_MESSAGE;
		} else if (variable === '' && (provider.api_key_env ?? '') !== '') {
			// Removing a variable is not expressible in the patch (`null` means unchanged).
			errors.apiKeyEnv = API_KEY_ENV_CLEAR_MESSAGE;
		}
	}
	return errors;
}

/** True when at least one field has a message. */
export function hasFormErrors(errors: object): boolean {
	return Object.values(errors).some((message) => message !== undefined);
}

// ─── Profiles: the whole list, sent only when it changed ─────────────────────

/** JSON with sorted keys, so two equal profiles compare equal whatever the key order. */
function stableJson(value: unknown): string {
	if (Array.isArray(value)) return `[${value.map(stableJson).join(',')}]`;
	if (isJsonObject(value)) {
		const entries = Object.keys(value)
			.sort()
			.map((key) => `${JSON.stringify(key)}:${stableJson(value[key])}`);
		return `{${entries.join(',')}}`;
	}
	return JSON.stringify(value) ?? 'null';
}

function editOf(profile: ModelProfileView): ReasoningProfileEdit {
	const label = profile.label?.trim() ?? '';
	return {
		id: profile.id,
		label: label === '' ? null : label,
		parameters: profile.parameters,
		remove: profile.remove
	};
}

export type ProfilesOutcome = { ok: true; edits: ReasoningProfileEdit[] | null } | { ok: false; message: string };

/**
 * The whole profile list as the engine receives it, or `null` when it is the list that was read.
 * A list that does not parse, or whose ids repeat, is refused and nothing is sent.
 */
export function profilesPatch(original: readonly ModelProfileView[], drafts: readonly ProfileDraft[]): ProfilesOutcome {
	const edits: ReasoningProfileEdit[] = [];
	for (const [index, draft] of drafts.entries()) {
		const others = drafts.filter((_, i) => i !== index).map((d) => d.id.trim());
		const parsed = parseProfileDraft(draft, others);
		if (!parsed.ok) {
			const first = parsed.errors.id ?? parsed.errors.parameters ?? parsed.errors.remove ?? '';
			const name = draft.id.trim() === '' ? 'sans identifiant' : `« ${draft.id.trim()} »`;
			return { ok: false, message: `Profil ${name} : ${first}` };
		}
		edits.push(parsed.value);
	}
	const unchanged =
		edits.length === original.length && edits.every((edit, i) => stableJson(edit) === stableJson(editOf(original[i])));
	return { ok: true, edits: unchanged ? null : edits };
}

// ─── Patches (`null` = unchanged, as in the Rust contract) ──────────────────

/**
 * A required text: the trimmed new value when it differs, else `null`. An empty value is never
 * sent (the form refuses it first), so no empty string reaches the engine for a required field.
 */
function changedRequired(before: string, after: string): string | null {
	const next = after.trim();
	return next === '' || next === before.trim() ? null : next;
}

/**
 * An optional text: the trimmed new value when it differs, else `null`. An explicit clear of a
 * value that was set is sent as `''`; an empty field that was already empty stays unchanged.
 */
function changedOptional(before: string, after: string): string | null {
	const next = after.trim();
	return next === before.trim() ? null : next;
}

/** The new count when it differs from `before` (an invalid text is left out: validate first). */
function changedCount(before: number, text: string): number | null {
	const next = parseCount(text);
	return next === null || next === before ? null : next;
}

/** Modalities in display order, unknown values after the known ones. */
function orderedModalities(values: readonly string[]): string[] {
	const known = MODALITY_OPTIONS.filter((m) => values.includes(m));
	const unknown = [...new Set(values)].filter((v) => !MODALITY_OPTIONS.includes(v)).sort();
	return [...known, ...unknown];
}

export function buildProviderPatch(provider: ProviderView, form: ProviderForm): ProviderPatch {
	return {
		name: changedRequired(provider.name, form.name),
		endpoint: changedRequired(provider.endpoint, form.endpoint),
		protocol: changedRequired(provider.protocol, form.protocol),
		auth: changedRequired(provider.auth, form.auth),
		auth_header: changedOptional(provider.auth_header ?? '', form.authHeader),
		// The engine refuses a variable on a provider that has an inline key: it is left alone there.
		api_key_env: provider.has_inline_key ? null : changedRequired(provider.api_key_env ?? '', form.apiKeyEnv)
	};
}

/**
 * The patch of a model. The context window is not edited in this form: its fields are never sent,
 * so the stored value is kept as it is. `profiles` is the whole list from `profilesPatch`, or `null`.
 */
export function buildModelPatch(
	model: ModelView,
	form: ModelForm,
	profiles: ReasoningProfileEdit[] | null = null
): ModelPatch {
	const modalitiesNow = orderedModalities(form.modalities);
	const modalitiesBefore = orderedModalities(model.input_modalities);
	const reasoningChanged = form.reasoningDefault !== model.reasoning_default;
	return {
		name: changedRequired(model.name, form.name),
		api_model: changedRequired(model.api_model, form.apiModel),
		max_input_tokens: changedCount(model.max_input_tokens, form.maxInput),
		max_output_tokens: changedCount(model.max_output_tokens, form.maxOutput),
		input_modalities: modalitiesNow.join('|') === modalitiesBefore.join('|') ? null : modalitiesNow,
		document_mime_types: changedDocumentTypes(model, form),
		pricing: changedPricing(model, form),
		reasoning_default: reasoningChanged && form.reasoningDefault !== null ? form.reasoningDefault : null,
		clear_reasoning_default: reasoningChanged && form.reasoningDefault === null,
		profiles
	};
}

/** The accepted document types when they changed; `[]` removes them with the document input. */
function changedDocumentTypes(model: ModelView, form: ModelForm): string[] | null {
	const next = form.modalities.includes('document') ? documentTypes(form.documentTypes) : [];
	return next.join('|') === model.document_mime_types.join('|') ? null : next;
}

function pricingOf(form: ModelForm): PricingEdit | null {
	const input = parsePrice(form.priceInput);
	const output = parsePrice(form.priceOutput);
	const cache = parsePrice(form.priceCache);
	return input === null || output === null || cache === null ? null : { input, output, cache_read: cache };
}

/** A decimal written without its insignificant zeros (`0.30` and `00.3` are both `0.3`). */
function decimalKey(value: string): string {
	const [int, frac = ''] = value.trim().split('.');
	const whole = int.replace(/^0+(?=\d)/, '');
	const fraction = frac.replace(/0+$/, '');
	return fraction === '' ? whole : `${whole}.${fraction}`;
}

/** The three prices when one of them differs from the file (an exact decimal compare). */
function changedPricing(model: ModelView, form: ModelForm): PricingEdit | null {
	const next = pricingOf(form);
	if (next === null) return null;
	const same = (a: string | null | undefined, b: string) => a != null && decimalKey(a) === decimalKey(b);
	const before = model.pricing;
	if (before && same(before.input, next.input) && same(before.output, next.output) && same(before.cache_read, next.cache_read)) {
		return null;
	}
	return next;
}

/** Every field of a new provider (the file has none of them yet). */
export function buildProviderCreatePatch(form: ProviderForm): ProviderPatch {
	const header = form.authHeader.trim();
	const variable = form.apiKeyEnv.trim();
	return {
		name: form.name.trim(),
		endpoint: form.endpoint.trim(),
		protocol: form.protocol,
		auth: form.auth,
		auth_header: header === '' ? null : header,
		api_key_env: variable === '' ? null : variable
	};
}

/** Every field of a new model. Validate first: the counts and prices must parse. */
export function buildModelCreatePatch(form: ModelForm, profiles: ReasoningProfileEdit[] | null = null): ModelPatch {
	const types = form.modalities.includes('document') ? documentTypes(form.documentTypes) : [];
	return {
		name: form.name.trim(),
		api_model: form.apiModel.trim(),
		max_input_tokens: parseCount(form.maxInput),
		max_output_tokens: parseCount(form.maxOutput),
		input_modalities: orderedModalities(form.modalities),
		document_mime_types: types.length === 0 ? null : types,
		pricing: pricingOf(form),
		reasoning_default: form.reasoningDefault,
		clear_reasoning_default: false,
		profiles: profiles && profiles.length > 0 ? profiles : null
	};
}

/** Models that have no price for input, output or cache read. */
export function unpricedModels(providers: readonly ProviderView[]): number {
	return providers
		.flatMap((p) => p.models)
		.filter((m) => !m.pricing || m.pricing.input == null || m.pricing.output == null || m.pricing.cache_read == null)
		.length;
}

export function isEmptyProviderPatch(patch: ProviderPatch): boolean {
	return Object.values(patch).every((value) => value === null);
}

export function isEmptyModelPatch(patch: ModelPatch): boolean {
	return Object.values(patch).every((value) => value === null || value === false);
}

// ─── Deletion and write outcomes ─────────────────────────────────────────────

/** What deleting a provider takes with it: its models, by name. */
export function providerDeletionDetail(provider: ProviderView): string {
	if (provider.models.length === 0) return 'Aucun modèle n’est déclaré pour ce fournisseur.';
	const names = provider.models.map((m) => `« ${displayName(m.name, m.id)} »`).join(', ');
	return `Ses modèles sont retirés avec lui : ${names}.`;
}

export type WriteOutcome = { ok: true } | { ok: false; message: string; conflict: boolean };

function isConflictError(error: unknown): boolean {
	return typeof error === 'object' && error !== null && 'code' in error && error.code === 'conflict';
}

/** The message to show for a failed write; a stale revision gets its own explanation. */
export function writeFailure(error: unknown): WriteOutcome {
	if (isConflictError(error)) return { ok: false, message: CONFLICT_MESSAGE, conflict: true };
	const message = error instanceof Error ? error.message : String(error);
	return { ok: false, message: message || 'Échec de l’enregistrement.', conflict: false };
}
