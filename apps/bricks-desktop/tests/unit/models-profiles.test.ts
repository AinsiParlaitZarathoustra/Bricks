// Unit tests for the reasoning levels of the model form: free ids, JSON parameters, JSON Pointers,
// duplicates, the level created from its name alone, and the whole list sent only when it changed.
import { describe, expect, it } from 'vitest';
import type { ModelView } from '../../src/lib/ipc/bindings/ModelView';
import {
	PROFILE_ID_DUPLICATE,
	PROFILE_ID_REQUIRED,
	PROFILE_PARAMETERS_OBJECT,
	buildModelPatch,
	draftLabel,
	isEmptyModelPatch,
	modelForm,
	newLevelDraft,
	newProfileDraft,
	parseProfileDraft,
	profileDraft,
	profilesPatch,
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
		expect(parseProfileDraft(draft({ id: ' ' }), [])).toMatchObject({ ok: false, errors: { id: PROFILE_ID_REQUIRED() } });
		expect(parseProfileDraft(draft({ id: 'low' }), ['low'])).toMatchObject({
			ok: false,
			errors: { id: PROFILE_ID_DUPLICATE() }
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
				errors: { parameters: PROFILE_PARAMETERS_OBJECT() }
			});
		}
	});

	it('refuses a JSON Pointer that does not start with a slash', () => {
		const result = parseProfileDraft(draft({ remove: 'thinking' }), []);
		expect(result.ok).toBe(false);
		if (!result.ok) expect(result.errors.remove).toBeDefined();
	});
});

describe('newLevelDraft', () => {
	it('names the reasoning effort in the request field of the protocol', () => {
		expect(JSON.parse(newLevelDraft('low', 'chat_completions').parameters)).toEqual({ reasoning_effort: 'low' });
		expect(JSON.parse(newLevelDraft('high', 'responses').parameters)).toEqual({ reasoning: { effort: 'high' } });
		expect(JSON.parse(newLevelDraft('max', 'anthropic_messages').parameters)).toEqual({ output_config: { effort: 'max' } });
	});

	it('takes the trimmed level as the id, with no label and no removed pointer', () => {
		const level = newLevelDraft('  max ', 'chat_completions');
		expect(level.id).toBe('max');
		expect(level.label).toBe('');
		expect(level.remove).toBe('');
	});

	it('gives a level that parses as it is, in the shape of the engine', () => {
		expect(parseProfileDraft(newLevelDraft('low', 'responses'), [])).toEqual({
			ok: true,
			value: { id: 'low', label: null, parameters: { reasoning: { effort: 'low' } }, remove: [] }
		});
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

	it('sends the whole list when a level was added, edited, removed or reordered', () => {
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

	it('sends an empty list when every level is removed', () => {
		expect(profilesPatch(model.profiles, [])).toEqual({ ok: true, edits: [] });
	});

	it('refuses the list as a whole when one level is invalid, naming it', () => {
		const drafts = [...model.profiles.map(profileDraft), draft({ id: 'broken', parameters: '{' })];
		const result = profilesPatch(model.profiles, drafts);
		expect(result.ok).toBe(false);
		if (!result.ok) expect(result.message).toContain('« broken »');
	});

	it('refuses duplicate ids in the list', () => {
		const drafts = [draft({ id: 'same' }), draft({ id: 'same', label: 'Autre' })];
		const result = profilesPatch([], drafts);
		expect(result.ok).toBe(false);
		if (!result.ok) expect(result.message).toContain(PROFILE_ID_DUPLICATE());
	});
});

describe('profile list in the model patch', () => {
	it('keeps the order of the levels in the patch', () => {
		const drafts = [profileDraft(model.profiles[1]), profileDraft(model.profiles[0])];
		const outcome = profilesPatch(model.profiles, drafts);
		const patch = buildModelPatch(model, modelForm(model), outcome.ok ? outcome.edits : null);
		expect(patch.profiles?.map((p) => p.id)).toEqual(['high', 'low']);
	});

	it('sends no profiles when the list did not change', () => {
		const patch = buildModelPatch(model, modelForm(model), null);
		expect(patch.profiles).toBeNull();
	});

	it('an untouched form with its levels read back gives an empty patch', () => {
		const outcome = profilesPatch(model.profiles, model.profiles.map(profileDraft));
		const patch = buildModelPatch(model, modelForm(model), outcome.ok ? outcome.edits : null);
		expect(isEmptyModelPatch(patch)).toBe(true);
	});
});

describe('labels of drafts', () => {
	it('shows the label, else the id', () => {
		expect(draftLabel(draft({ id: 'max', label: 'Maximum' }))).toBe('Maximum');
		expect(draftLabel(draft({ id: 'max', label: '  ' }))).toBe('max');
	});
});
