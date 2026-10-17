// Input history of the composer: ↑ recalls earlier prompts of the conversation,
// ↓ goes forward again and finally gives back the draft that was there before.
// The draft is never lost: the text it had when browsing started is kept aside.
// Pure (no runes): unit-tested. Owner: Haiku 3 (Sprint 14.1).

export interface CaretState {
  text: string
  selectionStart: number
  selectionEnd: number
  /** True while a recalled entry is shown. */
  browsing: boolean
}

/**
 * Whether ↑ recalls an older entry: in an empty editor, with the caret at the
 * very start, or on the first line while already browsing.
 */
export function shouldRecallOlder(c: CaretState): boolean {
  if (c.browsing) return !c.text.slice(0, c.selectionStart).includes('\n')
  return c.text === '' || (c.selectionStart === 0 && c.selectionEnd === 0)
}

/** Whether ↓ moves towards newer entries: only while browsing, on the last line. */
export function shouldRecallNewer(c: CaretState): boolean {
  return c.browsing && !c.text.slice(c.selectionEnd).includes('\n')
}

/** The draft kept aside when browsing started. */
export interface SavedDraft<D extends object> {
  owner: D
  text: string
}

export class HistoryCursor<D extends object = object> {
  #index: number | null = null
  #saved: SavedDraft<D> | null = null

  get browsing(): boolean {
    return this.#index !== null
  }

  /**
   * Shows the previous entry (the first ↑ also saves `draftText` of `owner`).
   * Returns `null` when there is no older entry: the caller leaves the key alone.
   */
  older(entries: readonly string[], draftText: string, owner: D): string | null {
    if (entries.length === 0) return null
    if (this.#index === null) {
      this.#saved = { owner, text: draftText }
      this.#index = entries.length - 1
    } else {
      const current = Math.min(this.#index, entries.length - 1)
      if (current === 0) {
        this.#index = 0
        return null
      }
      this.#index = current - 1
    }
    return entries[this.#index] ?? null
  }

  /**
   * Shows the next, newer entry. Past the newest one, returns the saved draft
   * and stops browsing. Returns `null` when not browsing (the key is left alone).
   */
  newer(entries: readonly string[]): string | null {
    if (this.#index === null) return null
    if (this.#index < entries.length - 1) {
      this.#index += 1
      return entries[this.#index] ?? null
    }
    const saved = this.#saved?.text ?? ''
    this.#index = null
    this.#saved = null
    return saved
  }

  /**
   * Stops browsing without moving (after a send, or when the conversation
   * changes). Returns the saved draft to put back, or `null` when not browsing.
   */
  end(): SavedDraft<D> | null {
    const saved = this.#index === null ? null : this.#saved
    this.#index = null
    this.#saved = null
    return saved
  }
}
