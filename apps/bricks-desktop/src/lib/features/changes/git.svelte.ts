// Git state of a project folder: status and the commit list.
// Owner: Haiku 7 (changes feature). Components call `refresh()` and
// `loadCommits()`; nothing here computes a diff.

import { errorMessage, gitLog, gitStatus, type GitCommit, type GitStatus } from '$lib/ipc/client'

/** Commits per page. */
export const COMMIT_PAGE = 30

export class GitState {
  readonly projectId: string
  status = $state<GitStatus | null>(null)
  statusError = $state<string | null>(null)
  statusLoading = $state(false)
  /** Bumped by each successful refresh: patches cached under an older value are stale. */
  revision = $state(0)
  commits = $state<GitCommit[]>([])
  commitsLoaded = $state(false)
  commitsLoading = $state(false)
  commitsError = $state<string | null>(null)
  commitsExhausted = $state(false)
  #statusSeq = 0

  constructor(projectId: string) {
    this.projectId = projectId
  }

  async refresh(): Promise<void> {
    const seq = ++this.#statusSeq
    this.statusLoading = true
    try {
      const status = await gitStatus(this.projectId)
      if (seq !== this.#statusSeq) return
      this.status = status
      this.statusError = null
      this.revision++
    } catch (e) {
      if (seq === this.#statusSeq) this.statusError = errorMessage(e)
    } finally {
      if (seq === this.#statusSeq) this.statusLoading = false
    }
  }

  /** The first page of commits, or the next one when `next` is set. */
  async loadCommits(next = false): Promise<void> {
    if (this.commitsLoading) return
    const skip = next ? this.commits.length : 0
    this.commitsLoading = true
    try {
      const page = await gitLog(this.projectId, COMMIT_PAGE, skip)
      this.commits = next ? [...this.commits, ...page] : page
      this.commitsExhausted = page.length < COMMIT_PAGE
      this.commitsError = null
      this.commitsLoaded = true
    } catch (e) {
      this.commitsError = errorMessage(e)
    } finally {
      this.commitsLoading = false
    }
  }
}

// Plain map: see journal.svelte.ts. The entries hold their own `$state`.
const repos = new Map<string, GitState>()

/** Git state of a project (created on first use). Safe in a derivation. */
export function gitState(projectId: string): GitState {
  let state = repos.get(projectId)
  if (!state) {
    state = new GitState(projectId)
    repos.set(projectId, state)
  }
  return state
}
