// The shared `/` command list, loaded once from the engine registry
// (`slash_commands`) and cached. A failed load is shown and can be retried.
// Owner: Haiku 3 (Sprint 14.1).

import { errorMessage, slashCommands } from '$lib/ipc/client'
import type { SlashCommandInfo } from '$lib/ipc/bindings/SlashCommandInfo'

export type SlashRegistryState =
  | { status: 'loading' }
  | { status: 'ready'; commands: SlashCommandInfo[] }
  | { status: 'error'; message: string }

class SlashRegistry {
  state = $state<SlashRegistryState>({ status: 'loading' })
  #pending: Promise<void> | null = null

  get commands(): SlashCommandInfo[] | null {
    return this.state.status === 'ready' ? this.state.commands : null
  }

  /** Loads the list once; concurrent callers share the same request. */
  load(): Promise<void> {
    if (this.state.status === 'ready') return Promise.resolve()
    this.#pending ??= this.#fetch()
    return this.#pending
  }

  /** Tries again after an error. */
  retry(): Promise<void> {
    this.state = { status: 'loading' }
    this.#pending = null
    return this.load()
  }

  async #fetch(): Promise<void> {
    try {
      const commands = await slashCommands()
      this.state = { status: 'ready', commands }
    } catch (e) {
      this.state = { status: 'error', message: errorMessage(e) }
    } finally {
      this.#pending = null
    }
  }
}

export const slashRegistry = new SlashRegistry()
