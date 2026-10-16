// Pure helpers of the « Modèles » settings: form state, client validation,
// patch building, labels and write-failure mapping. No state, no IPC: the
// write calls live in ModelSettings.svelte. Rust validates the rest.

import type { ModelPatch } from '$lib/ipc/bindings/ModelPatch';
import type { ModelProfileView } from '$lib/ipc/bindings/ModelProfileView';
import type { ModelView } from '$lib/ipc/bindings/ModelView';
import type { ProviderPatch } from '$lib/ipc/bindings/ProviderPatch';
import type { ProviderView } from '$lib/ipc/bindings/ProviderView';
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
export const MODALITY_OPTIONS: readonly string[] = ['text', 'image', 'pdf', 'audio', 'video', 'file'];

export const CONFLICT_MESSAGE =
	'Le fichier des fournisseurs a été modifié ailleurs. Vos modifications n’ont pas été enregistrées : les valeurs ont été rechargées, vérifiez-les avant d’enregistrer à nouveau.';

export const COUNT_ERROR = 'Saisissez un nombre entier supérieur à 0.';
export const REQUIRED_MESSAGE = 'Ce champ est requis.';
export const API_KEY_ENV_REQUIRED_MESSAGE =
	'Indiquez la variable qui contient la clé : elle est requise avec cette authentification.';
export const API_KEY_ENV_CLEAR_MESSAGE = 'Cette variable ne peut pas être effacée depuis cette fenêtre.';

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

// ─── Forms ───────────────────────────────────────────────────────────────────

export interface ProviderForm {
	name: string;
	endpoint: string;
	protocol: string;
	auth: string;
	authHeader: string;
	apiKeyEnv: string;
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

export interface ModelForm {
	name: string;
	apiModel: string;
	maxInput: string;
	maxOutput: string;
	/** Empty when the context window is unknown. */
	contextWindow: string;
	modalities: string[];
	/** Default reasoning profile id, `null` for none. */
	reasoningDefault: string | null;
}

export function modelForm(model: ModelView): ModelForm {
	return {
		name: model.name,
		apiModel: model.api_model,
		maxInput: String(model.max_input_tokens),
		maxOutput: String(model.max_output_tokens),
		contextWindow: model.context_window_tokens === null ? '' : String(model.context_window_tokens),
		modalities: [...model.input_modalities],
		reasoningDefault: model.reasoning_default
	};
}

// ─── Validation ──────────────────────────────────────────────────────────────

/** A whole number above zero typed by the user (spaces and underscores allowed), or `null`. */
export function parseCount(text: string): number | null {
	const digits = text.replace(/[\s_  ]/g, '');
	if (!/^\d+$/.test(digits)) return null;
	const value = Number(digits);
	return Number.isSafeInteger(value) && value > 0 ? value : null;
}

export interface ModelFormErrors {
	name?: string;
	apiModel?: string;
	maxInput?: string;
	maxOutput?: string;
	contextWindow?: string;
}

export interface ProviderFormErrors {
	name?: string;
	endpoint?: string;
	apiKeyEnv?: string;
}

/**
 * Client checks: required texts are filled, token counts are whole numbers above zero, and the
 * key variable is named when the authentication needs one. Rust checks the rest.
 */
export function validateModelForm(form: ModelForm): ModelFormErrors {
	const errors: ModelFormErrors = {};
	if (form.name.trim() === '') errors.name = REQUIRED_MESSAGE;
	if (form.apiModel.trim() === '') errors.apiModel = REQUIRED_MESSAGE;
	if (parseCount(form.maxInput) === null) errors.maxInput = COUNT_ERROR;
	if (parseCount(form.maxOutput) === null) errors.maxOutput = COUNT_ERROR;
	if (form.contextWindow.trim() !== '' && parseCount(form.contextWindow) === null) {
		errors.contextWindow = COUNT_ERROR;
	}
	return errors;
}

export function validateProviderForm(form: ProviderForm, provider: ProviderView): ProviderFormErrors {
	const errors: ProviderFormErrors = {};
	if (form.name.trim() === '') errors.name = REQUIRED_MESSAGE;
	if (form.endpoint.trim() === '') errors.endpoint = REQUIRED_MESSAGE;
	// A provider with an inline key has no variable to name: the field is disabled there.
	if (!provider.has_inline_key) {
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

export function buildModelPatch(model: ModelView, form: ModelForm): ModelPatch {
	const contextUnknown = form.contextWindow.trim() === '';
	const context = contextUnknown ? null : parseCount(form.contextWindow);
	const modalitiesNow = orderedModalities(form.modalities);
	const modalitiesBefore = orderedModalities(model.input_modalities);
	const reasoningChanged = form.reasoningDefault !== model.reasoning_default;
	return {
		name: changedRequired(model.name, form.name),
		api_model: changedRequired(model.api_model, form.apiModel),
		max_input_tokens: changedCount(model.max_input_tokens, form.maxInput),
		max_output_tokens: changedCount(model.max_output_tokens, form.maxOutput),
		context_window_tokens: context !== null && context !== model.context_window_tokens ? context : null,
		clear_context_window: contextUnknown && model.context_window_tokens !== null,
		input_modalities: modalitiesNow.join('|') === modalitiesBefore.join('|') ? null : modalitiesNow,
		reasoning_default: reasoningChanged && form.reasoningDefault !== null ? form.reasoningDefault : null,
		clear_reasoning_default: reasoningChanged && form.reasoningDefault === null
	};
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
