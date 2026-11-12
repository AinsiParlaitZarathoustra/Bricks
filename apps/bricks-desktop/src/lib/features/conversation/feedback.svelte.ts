// « Bonne réponse » / « Mauvaise réponse » on a complete response, kept on this
// machine (the webview's storage), per conversation and response. Unreadable
// or blocked storage leaves every response unrated; nothing else depends on it.

export type Rating = 'good' | 'bad'

const STORAGE_KEY = 'bricks.response-ratings'

function readAll(): Record<string, Rating> {
	try {
		const raw = globalThis.localStorage?.getItem(STORAGE_KEY)
		const parsed: unknown = raw ? JSON.parse(raw) : {}
		if (!parsed || typeof parsed !== 'object') return {}
		const out: Record<string, Rating> = {}
		for (const [key, value] of Object.entries(parsed)) {
			if (value === 'good' || value === 'bad') out[key] = value
		}
		return out
	} catch {
		return {}
	}
}

class Ratings {
	#all = $state<Record<string, Rating>>(readAll())

	/** The key of one response: its conversation and the item that ends it. */
	key(sessionId: string, anchorId: string): string {
		return `${sessionId}:${anchorId}`
	}

	get(key: string): Rating | undefined {
		return this.#all[key]
	}

	/** Sets a rating, or clears it when `rating` is null. */
	set(key: string, rating: Rating | null) {
		const next = { ...this.#all }
		if (rating) next[key] = rating
		else delete next[key]
		this.#all = next
		try {
			globalThis.localStorage?.setItem(STORAGE_KEY, JSON.stringify(next))
		} catch {
			// Storage refused: the rating holds for this session only.
		}
	}
}

export const ratings = new Ratings()
