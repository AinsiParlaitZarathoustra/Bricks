// @vitest-environment happy-dom
// The hover card of a grain in the message rail (MessageRail.svelte): hovered or focused, it
// shows the message, the start of its answer as plain text, its place among the marks and, when
// known, its date. Mounted in happy-dom; the interface is in French (setup-language.ts).
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { flushSync, mount, unmount } from 'svelte'
import MessageRail from '../../src/lib/features/conversation/MessageRail.svelte'

interface Mark {
	key: string
	label: string
	answer: string
	at: number | null
	place?: number
	total?: number
}

function mark(index: number, overrides: Partial<Mark> = {}): Mark {
	return { key: `turn-${index}`, label: `Message ${index + 1}`, answer: '', at: null, ...overrides }
}

let target: HTMLDivElement
let app: ReturnType<typeof mount> | null = null

function show(marks: Mark[]): void {
	// Unless a test says otherwise, the marks are all the messages of the conversation.
	const full = marks.map((m, i) => ({ place: i + 1, total: marks.length, ...m }))
	app = mount(MessageRail, { target, props: { marks: full, scroller: null } })
	flushSync()
}

/** The hover card of the grain for `key`: the aria-hidden span directly inside its button. */
function card(key: string): HTMLElement {
	const el = target.querySelector<HTMLElement>(`[data-mark="${key}"] > span[aria-hidden="true"]`)
	if (!el) throw new Error(`no card for ${key}`)
	return el
}

/** The footer of the card: the place, then the date when known. */
function footer(key: string): HTMLElement {
	const el = card(key).querySelector<HTMLElement>('.tabular-nums')
	if (!el) throw new Error(`no footer for ${key}`)
	return el
}

/** Visible text, with the narrow and non-breaking spaces read as plain spaces. */
const text = (el: Element) => (el.textContent ?? '').replace(/[  ]/g, ' ').replace(/\s+/g, ' ').trim()

beforeEach(() => {
	target = document.createElement('div')
	document.body.appendChild(target)
})

afterEach(() => {
	if (app) void unmount(app)
	app = null
	target.remove()
})

describe('the hover card of a grain', () => {
	it('shows the message, and the grain has no title tooltip', () => {
		show([mark(0, { label: 'Première question' }), mark(1)])
		const button = target.querySelector<HTMLButtonElement>('[data-mark="turn-0"]')
		expect(button?.getAttribute('aria-label')).toBe('Première question · 1 sur 2')
		expect(button?.hasAttribute('title')).toBe(false)
		expect(card('turn-0').getAttribute('aria-hidden')).toBe('true')
		expect(text(card('turn-0'))).toContain('Première question')
	})

	it('shows the answer as plain text, without Markdown marks', () => {
		show([mark(0, { answer: '**gras** et `code`' }), mark(1)])
		expect(text(card('turn-0'))).toContain('gras et code')
		expect(card('turn-0').textContent).not.toMatch(/[*`]/)
	})

	it('reads a link as its text and drops code blocks', () => {
		show([mark(0, { answer: 'Voir [la doc](https://example.com)\n\n```ts\nconst x = 1\n```\n## Fin' }), mark(1)])
		const shown = text(card('turn-0'))
		expect(shown).toContain('Voir la doc')
		expect(shown).toContain('Fin')
		expect(shown).not.toContain('https://example.com')
		expect(shown).not.toContain('const x')
	})

	it('numbers the place among the marks, in French', () => {
		show([mark(0), mark(1), mark(2)])
		expect(text(footer('turn-1'))).toBe('2 sur 3')
		expect(text(footer('turn-0'))).toBe('1 sur 3')
		expect(text(footer('turn-2'))).toBe('3 sur 3')
	})

	it('numbers by the place in the whole conversation, not among the loaded messages', () => {
		show([mark(0, { place: 161, total: 200 }), mark(1, { place: 162, total: 200 })])
		expect(text(footer('turn-0'))).toBe('161 sur 200')
		const grain = target.querySelector('[data-mark="turn-1"]')
		expect(grain?.getAttribute('aria-label')).toContain('162 sur 200')
	})

	it('keeps the characters that are not Markdown', () => {
		show([mark(0, { answer: 'Voir bricks_foundation.txt et ~/.bricks, a > b, C# et 2 * 3' }), mark(1)])
		expect(text(card('turn-0'))).toContain('Voir bricks_foundation.txt et ~/.bricks, a > b, C# et 2 * 3')
	})

	it('shows the date of a message whose time is known', () => {
		// Local midday: the day and the hour read the same in any time zone.
		const at = new Date(2026, 0, 15, 12, 30).getTime()
		show([mark(0, { at }), mark(1)])
		const shown = text(footer('turn-0'))
		expect(shown).toContain('1 sur 2')
		expect(shown).toContain('15 janvier 2026')
		expect(shown).toContain('12:30')
		expect(shown).not.toContain('Invalid')
	})

	it('shows no answer line when the answer is empty', () => {
		show([mark(0, { answer: '' }), mark(1, { answer: 'Réponse' })])
		expect(card('turn-0').querySelector('.line-clamp-3')).toBeNull()
		expect(text(card('turn-1'))).toContain('Réponse')
	})

	it('shows no date when the time of the message is unknown', () => {
		show([mark(0, { at: null }), mark(1)])
		expect(text(footer('turn-0'))).toBe('1 sur 2')
	})
})
