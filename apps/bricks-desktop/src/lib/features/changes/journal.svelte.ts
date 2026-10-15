// Change journal caches: the project history and each conversation's own
// records. Both stay current through `change_recorded` events.
// Owner: Haiku 7 (changes feature).

import {
  changeLogInfo,
  changeLogList,
  errorMessage,
  type ChangeJournalInfo,
  type ChangeRecord,
} from '$lib/ipc/client'
import { onChangeRecorded } from '$lib/state/app.svelte'

/** Records per page of the project history. */
export const HISTORY_PAGE = 50

const newestFirst = (a: ChangeRecord, b: ChangeRecord) => b.at - a.at

export class ProjectJournal {
  readonly projectId: string
  records = $state<ChangeRecord[]>([])
  info = $state<ChangeJournalInfo | null>(null)
  /** Size of the page currently shown. */
  limit = $state(HISTORY_PAGE)
  loaded = $state(false)
  loading = $state(false)
  error = $state<string | null>(null)
  /** The last page was shorter than asked: nothing older to load. */
  exhausted = $state(false)
  #seq = 0

  constructor(projectId: string) {
    this.projectId = projectId
  }

  /** Loads the newest `limit` records and the journal's header. */
  async load(limit = this.limit): Promise<void> {
    const seq = ++this.#seq
    this.loading = true
    try {
      const [records, info] = await Promise.all([
        changeLogList(this.projectId, null, limit),
        changeLogInfo(this.projectId),
      ])
      if (seq !== this.#seq) return
      this.records = [...records].sort(newestFirst)
      this.info = info
      this.limit = limit
      this.exhausted = records.length < limit
      this.error = null
      this.loaded = true
    } catch (e) {
      if (seq === this.#seq) this.error = errorMessage(e)
    } finally {
      if (seq === this.#seq) this.loading = false
    }
  }

  more(): Promise<void> {
    return this.load(this.limit + HISTORY_PAGE)
  }

  add(record: ChangeRecord): void {
    if (this.records.some((r) => r.id === record.id)) return
    this.records = [record, ...this.records].sort(newestFirst)
  }
}

export class SessionJournal {
  readonly projectId: string
  readonly sessionId: string
  records = $state<ChangeRecord[]>([])
  loaded = $state(false)
  loading = $state(false)
  error = $state<string | null>(null)
  #seq = 0

  constructor(projectId: string, sessionId: string) {
    this.projectId = projectId
    this.sessionId = sessionId
  }

  async load(): Promise<void> {
    const seq = ++this.#seq
    this.loading = true
    try {
      const records = await changeLogList(this.projectId, this.sessionId)
      if (seq !== this.#seq) return
      this.records = [...records].sort(newestFirst)
      this.error = null
      this.loaded = true
    } catch (e) {
      if (seq === this.#seq) this.error = errorMessage(e)
    } finally {
      if (seq === this.#seq) this.loading = false
    }
  }

  add(record: ChangeRecord): void {
    if (this.records.some((r) => r.id === record.id)) return
    this.records = [record, ...this.records].sort(newestFirst)
  }
}

// Plain maps on purpose: lookups must work inside `$derived` without writing
// reactive state. The entries hold their own `$state`.
const projects = new Map<string, ProjectJournal>()
const sessions = new Map<string, SessionJournal>()
let subscribed = false

function subscribe(): void {
  if (subscribed) return
  subscribed = true
  onChangeRecorded((record) => {
    projects.get(record.project_id)?.add(record)
    sessions.get(record.session_id)?.add(record)
  })
}

/** The history of a project (created on first use). Safe in a derivation. */
export function projectJournal(projectId: string): ProjectJournal {
  subscribe()
  let journal = projects.get(projectId)
  if (!journal) {
    journal = new ProjectJournal(projectId)
    projects.set(projectId, journal)
  }
  return journal
}

/** The records of one conversation (created on first use). Safe in a derivation. */
export function sessionJournal(projectId: string, sessionId: string): SessionJournal {
  subscribe()
  let journal = sessions.get(sessionId)
  if (!journal) {
    journal = new SessionJournal(projectId, sessionId)
    sessions.set(sessionId, journal)
  }
  return journal
}
