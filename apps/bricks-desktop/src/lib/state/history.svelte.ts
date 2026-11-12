// The conversations visited, as in a browser: « Précédent » goes back to the one
// shown before, « Suivant » walks forward again. Opening a conversation by any
// other way drops the forward part. A new, still empty conversation is not a
// stop of its own. A conversation that can no longer be opened is skipped.

import { app } from './app.svelte'

const LIMIT = 100

class ConversationHistory {
	#stops = $state<string[]>([])
	#index = $state(-1)
	/** Set while the history itself opens a conversation, so that the visit is not recorded again. */
	#moving = false

	get canGoBack(): boolean {
		return this.#index > 0
	}

	get canGoForward(): boolean {
		return this.#index >= 0 && this.#index < this.#stops.length - 1
	}

	/** Records the conversation now shown (called whenever the shown conversation changes). */
	visit(sessionId: string | null) {
		if (this.#moving || sessionId === null) return
		if (this.#stops[this.#index] === sessionId) return
		const kept = this.#stops.slice(0, this.#index + 1)
		kept.push(sessionId)
		const overflow = Math.max(0, kept.length - LIMIT)
		this.#stops = kept.slice(overflow)
		this.#index = this.#stops.length - 1
	}

	back() {
		return this.#go(-1)
	}

	forward() {
		return this.#go(1)
	}

	async #go(step: -1 | 1) {
		let target = this.#index + step
		while (target >= 0 && target < this.#stops.length) {
			const sessionId = this.#stops[target]
			this.#moving = true
			try {
				const opened = await app.openSession(sessionId)
				if (opened) {
					this.#index = target
					return
				}
			} finally {
				this.#moving = false
			}
			// Gone or not openable any more: forget it and try the next stop the same way.
			const removed = target
			this.#stops = this.#stops.filter((_, i) => i !== removed)
			if (removed < this.#index) this.#index -= 1
			// Going back, the next candidate is the one before; going forward, it slid into `removed`.
			if (step < 0) target = removed - 1
		}
	}
}

export const conversationHistory = new ConversationHistory()
