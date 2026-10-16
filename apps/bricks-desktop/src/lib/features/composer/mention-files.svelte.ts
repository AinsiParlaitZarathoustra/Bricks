// The project file index behind `@` mentions: a short pause after typing, then
// one search for the text after `@`; « Actualiser » (Ctrl+R) rebuilds the index.
// Answers that arrive after a newer request are dropped. Owner: Haiku 3
// (Sprint 14.1, @ mentions).

import { errorMessage, projectFiles, type ProjectFileEntry } from '$lib/ipc/client'

export type MentionStatus = 'idle' | 'loading' | 'ready' | 'error'

const DEBOUNCE_MS = 120
const LIMIT = 50

export class MentionFiles {
  status = $state<MentionStatus>('idle')
  items = $state<ProjectFileEntry[]>([])
  error = $state('')

  #key: string | null = null
  #seq = 0
  #timer: ReturnType<typeof setTimeout> | null = null

  /** Searches after a short pause. The same search already answered is not repeated. */
  search(projectId: string, query: string): void {
    const key = `${projectId}\n${query}`
    if (key === this.#key && this.status !== 'error') return
    this.#key = key
    this.#cancelTimer()
    this.#timer = setTimeout(() => {
      this.#timer = null
      void this.#load(projectId, query, false)
    }, DEBOUNCE_MS)
  }

  /** Rebuilds the index now, with `refresh = true`. */
  refresh(projectId: string, query: string): void {
    this.#cancelTimer()
    this.#key = `${projectId}\n${query}`
    void this.#load(projectId, query, true)
  }

  /** Drops a pending search and any answer still on its way. */
  dispose(): void {
    this.#cancelTimer()
    this.#seq += 1
  }

  #cancelTimer() {
    if (this.#timer !== null) clearTimeout(this.#timer)
    this.#timer = null
  }

  async #load(projectId: string, query: string, refresh: boolean): Promise<void> {
    const seq = ++this.#seq
    this.status = 'loading'
    try {
      const items = await projectFiles(projectId, query, LIMIT, refresh)
      if (seq !== this.#seq) return
      this.items = items
      this.error = ''
      this.status = 'ready'
    } catch (e) {
      if (seq !== this.#seq) return
      this.items = []
      this.error = errorMessage(e)
      this.status = 'error'
    }
  }
}
