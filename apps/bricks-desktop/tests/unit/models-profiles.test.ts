// Unit tests for the reasoning profiles of the model form: free ids, JSON parameters, JSON Pointers,
// duplicates, the default profile, and the whole list sent only when it changed.
import { describe, expect, it } from 'vitest';
import type { ModelView } from '../../src/lib/ipc/bindings/ModelView';
import {
	DEFAULT_PROFILE_PENDING,
	PROFILE_ID_DUPLICATE,
	PROFILE_ID_REQUIRED,
	PROFILE_PARAMETERS_OBJECT,
	buildModelPatch,
	draftLabel,
	hasFormErrors,
	isEmptyModelPatch,
	modelForm,
	newProfileDraft,
	parseProfileDraft,
	profileDraft,
	profilesPatch,
	validateModelForm,
	type ModelForm,
	type ProfileDraft
} from '../../src/lib/features/settings/models';

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
	reasoning_default: 'low',
	profiles: [
		{ id: 'low', label: 'Léger', parameters: { budget: 1024 }, remove: ['/thinking'] },
		{ id: 'high', label: null, parameters: {}, remove: [] }
	],
	priced: false,
	document_mime_types: [],
	pricing: { input: '0.30', output: '1.20', cache_read: '0.03' }
};

function draft(overrides: Partial<ProfileDraft> = {}): ProfileDraft {
	return { ...newProfileDraft(), id: 'custom', ...overrides };
}

describe('parseProfileDraft', () => {
	it('accepts a free id and an optional label, and turns the texts into the engine shape', () => {
		const parsed = parseProfileDraft(
			draft({ id: '  extra-deep ', label: ' Très profond ', parameters: '{"effort": "max"}', remove: '/a\n\n /b/c ' }),
			[]
		);
		expect(parsed).toEqual({
			ok: true,
			value: {
				id: 'extra-deep',
				label: 'Très profond',
				parameters: { effort: 'max' },
				remove: ['/a', '/b/c']
			}
		});
	});

	it('sends a blank label as null and empty parameters as an empty object', () => {
		const parsed = parseProfileDraft(draft({ label: '  ', parameters: '   ' }), []);
		expect(parsed).toMatchObject({ ok: true, value: { label: null, parameters: {} } });
	});

	it('refuses an empty id and an id already taken by another profile', () => {
		expect(parseProfileDraft(draft({ id: ' ' }), [])).toMatchObject({ ok: false, errors: { id: PROFILE_ID_REQUIRED } });
		expect(parseProfileDraft(draft({ id: 'low' }), ['low'])).toMatchObject({
			ok: false,
			errors: { id: PROFILE_ID_DUPLICATE }
		});
	});

	it('refuses invalid JSON with the parser message, and keeps nothing to write', () => {
		const result = parseProfileDraft(draft({ parameters: '{"effort": ' }), []);
		expect(result.ok).toBe(false);
		if (!result.ok) expect(result.errors.parameters).toMatch(/^JSON invalide : /);
	});

	it('refuses JSON that is not an object (arrays, numbers, null)', () => {
		for (const text of ['[1, 2]', '3', 'null', '"x"']) {
			expect(parseProfileDraft(draft({ parameters: text }), [])).toMatchObject({
				ok: false,
				errors: { parameters: PROFILE_PARAMETERS_OBJECT }
			});
		}
	});

	it('refuses a JSON Pointer that does not start with a slash', () => {
		const result = parseProfileDraft(draft({ remove: 'thinking' }), []);
		expect(result.ok).toBe(false);
		if (!result.ok) expect(result.errors.remove).toBeDefined();
	});
});

describe('profilesPatch', () => {
	it('sends nothing when the list is the one that was read', () => {
		const drafts = model.profiles.map(profileDraft);
		expect(profilesPatch(model.profiles, drafts)).toEqual({ ok: true, edits: null });
	});

	it('does not count a different key order in the JSON as a change', () => {
		const original = [{ id: 'a', label: null, parameters: { x: 1, y: 2 }, remove: [] }];
		const reordered = [{ id: 'a', label: '', parameters: '{"y": 2, "x": 1}', remove: '' }];
		expect(profilesPatch(original, reordered)).toEqual({ ok: true, edits: null });
	});

	it('sends the whole list when a profile was added, edited, removed or reordered', () => {
		const drafts = model.profiles.map(profileDraft);
		const added = profilesPatch(model.profiles, [...drafts, draft({ id: 'max' })]);
		expect(added.ok && added.edits?.map((e) => e.id)).toEqual(['low', 'high', 'max']);

		const edited = profilesPatch(model.profiles, [{ ...drafts[0], label: 'Court' }, drafts[1]]);
		expect(edited.ok && edited.edits?.[0]).toMatchObject({ id: 'low', label: 'Court' });

		const removed = profilesPatch(model.profiles, [drafts[1]]);
		expect(removed.ok && removed.edits?.map((e) => e.id)).toEqual(['high']);

		const reordered = profilesPatch(model.profiles, [drafts[1], drafts[0]]);
		expect(reordered.ok && reordered.edits?.map((e) => e.id)).toEqual(['high', 'low']);
	});

	it('sends an empty list when every profile is removed', () => {
		expect(profilesPatch(model.profiles, [])).toEqual({ ok: true, edits: [] });
	});

	it('refuses the list as a whole when one profile is invalid, naming it', () => {
		const drafts = [...model.profiles.map(profileDraft), draft({ id: 'broken', parameters: '{' })];
		const result = profilesPatch(model.profiles, drafts);
		expect(result.ok).toBe(false);
		if (!result.ok) expect(result.message).toContain('« broken »');
	});

	it('refuses duplicate ids in the list', () => {
		const drafts = [draft({ id: 'same' }), draft({ id: 'same', label: 'Autre' })];
		const result = profilesPatch([], drafts);
		expect(result.ok).toBe(false);
		if (!result.ok) expect(result.message).toContain(PROFILE_ID_DUPLICATE);
	});
});

describe('default profile', () => {
	function formWith(overrides: Partial<ModelForm>): ModelForm {
		return { ...modelForm(model), ...overrides };
	}

	it('accepts a default among the profiles, or none', () => {
		expect(hasFormErrors(validateModelForm(formWith({ reasoningDefault: 'high' })))).toBe(false);
		expect(hasFormErrors(validateModelForm(formWith({ reasoningDefault: null })))).toBe(false);
	});

	it('refuses a default that is no longer among the profiles', () => {
		const errors = validateModelForm(formWith({ profiles: [profileDraft(model.profiles[1])] }));
		expect(errors.defaultProfile).toBe(DEFAULT_PROFILE_PENDING);
	});

	it('refuses the form while the removed or renamed default has no new choice', () => {
		expect(validateModelForm(formWith({ defaultPending: true })).defaultProfile).toBe(DEFAULT_PROFILE_PENDING);
	});

	it('clears the default in the same patch when the profile list loses it', () => {
		const form = formWith({
			profiles: [profileDraft(model.profiles[1])],
			reasoningDefault: null,
			defaultPending: false
		});
		const outcome = profilesPatch(model.profiles, form.profiles);
		expect(outcome.ok).toBe(true);
		const patch = buildModelPatch(model, form, outcome.ok ? outcome.edits : null);
		expect(patch).toMatchObject({ clear_reasoning_default: true, reasoning_default: null });
		expect(patch.profiles?.map((p) => p.id)).toEqual(['high']);
	});

	it('sends the new default with the list when the user picks another profile', () => {
		const form = formWith({ profiles: [profileDraft(model.profiles[1])], reasoningDefault: 'high' });
		const outcome = profilesPatch(model.profiles, form.profiles);
		const patch = buildModelPatch(model, form, outcome.ok ? outcome.edits : null);
		expect(patch).toMatchObject({ reasoning_default: 'high', clear_reasoning_default: false });
	});

	it('sends nothing for profiles when neither the list nor the default changed', () => {
		const patch = buildModelPatch(model, modelForm(model), null);
		expect(patch.profiles).toBeNull();
		expect(isEmptyModelPatch(patch)).toBe(true);
	});
});

describe('labels of drafts', () => {
	it('shows the label, else the id', () => {
		expect(draftLabel(draft({ id: 'max', label: 'Maximum' }))).toBe('Maximum');
		expect(draftLabel(draft({ id: 'max', label: '  ' }))).toBe('max');
	});
});
