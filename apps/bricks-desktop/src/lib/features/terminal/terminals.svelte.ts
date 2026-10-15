// Terminal sessions of the bottom pane. They live here, not in the component:
// the pane unmounts when it is closed, but the shells, their scrollback and
// their xterm instances survive, and the component reattaches them on mount.
// Each terminal belongs to the project it was opened for.
// Owner: Haiku 7 (terminal feature).

import { Terminal } from '@xterm/xterm'
import { FitAddon } from '@xterm/addon-fit'
import { SvelteMap } from 'svelte/reactivity'
import { errorMessage, ptyKill, ptyOpen, ptyResize, ptyWrite, type PtyEvent } from '$lib/ipc/client'
import { app } from '$lib/state/app.svelte'
import { terminalFont, terminalTheme } from './terminal-theme'

/** Lines kept per terminal. */
export const SCROLLBACK = 5000
/** Quiet time before a size change is sent to the PTY. */
const RESIZE_DELAY_MS = 120

// xterm's own screen-reader strings are English by default: set them in French
// (static, shared by every terminal).
Terminal.strings.promptLabel = 'Saisie du terminal'
Terminal.strings.tooMuchOutput = 'Trop de sortie : lecture de chaque ligne désactivée'

export type TerminalStatus = 'idle' | 'starting' | 'running' | 'exited' | 'error'

function decodeBase64(data: string): Uint8Array {
  const binary = atob(data)
  const bytes = new Uint8Array(binary.length)
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i)
  return bytes
}

export class TerminalSession {
  readonly id = crypto.randomUUID()
  readonly projectId: string
  readonly sessionId: string | null
  readonly term: Terminal
  readonly fit: FitAddon
  /** Element the xterm instance draws into. Moved between mounts. */
  readonly host: HTMLDivElement
  status = $state<TerminalStatus>('idle')
  /** Working directory the shell started in (from the PTY). */
  cwd = $state('')
  exitCode = $state<number | null>(null)
  error = $state<string | null>(null)
  #ptyId: string | null = null
  #opened = false
  #closing = false
  #resizeTimer: ReturnType<typeof setTimeout> | null = null

  constructor(projectId: string, sessionId: string | null) {
    this.projectId = projectId
    this.sessionId = sessionId
    this.host = document.createElement('div')
    this.host.className = 'h-full w-full'
    this.term = new Terminal({
      // Exposes the output to screen readers (NVDA, VoiceOver).
      screenReaderMode: true,
      scrollback: SCROLLBACK,
      cursorBlink: true,
      fontSize: 13,
      lineHeight: 1.2,
      fontFamily: terminalFont(),
      theme: terminalTheme(),
    })
    this.fit = new FitAddon()
    this.term.loadAddon(this.fit)
    this.term.onData((data) => this.#input(data))
    this.term.onResize(({ cols, rows }) => this.#scheduleResize(cols, rows))
  }

  /** Shows the terminal inside `container`, opening xterm on first use. */
  attach(container: HTMLElement): void {
    container.appendChild(this.host)
    if (!this.#opened) {
      this.term.open(this.host)
      this.#opened = true
    }
    this.fit.fit()
  }

  /** Re-measures the terminal after its container changed size. */
  refit(): void {
    if (this.#opened) this.fit.fit()
  }

  focus(): void {
    this.term.focus()
  }

  /** Starts the shell in the project folder. Does nothing while one runs. */
  start(): void {
    if (this.status === 'starting' || this.status === 'running') return
    this.status = 'starting'
    this.error = null
    this.exitCode = null
    ptyOpen(
      {
        project_id: this.projectId,
        session_id: this.sessionId,
        cols: this.term.cols,
        rows: this.term.rows,
      },
      (event) => this.#onEvent(event),
    )
      .then((info) => {
        this.#ptyId = info.id
        this.cwd = info.cwd
        if (this.#closing) {
          void ptyKill(info.id).catch(() => {})
          return
        }
        // Output or exit may have arrived before this reply.
        if (this.status === 'starting') this.status = 'running'
        this.#sendSize(this.term.cols, this.term.rows)
      })
      .catch((e: unknown) => {
        if (this.#closing) return
        this.status = 'error'
        this.error = errorMessage(e)
      })
  }

  /** Starts a new shell after the previous one exited or failed. */
  restart(): void {
    if (this.status !== 'exited' && this.status !== 'error') return
    this.#ptyId = null
    this.term.write('\r\n')
    this.start()
  }

  /** Kills the shell and releases xterm. The session cannot be used after. */
  dispose(): void {
    this.#closing = true
    if (this.#resizeTimer) clearTimeout(this.#resizeTimer)
    const id = this.#ptyId
    this.#ptyId = null
    if (id) void ptyKill(id).catch(() => {})
    this.term.dispose()
    this.host.remove()
  }

  #onEvent(event: PtyEvent): void {
    if (event.kind === 'data') {
      this.term.write(decodeBase64(event.data))
      return
    }
    this.status = 'exited'
    this.exitCode = event.code
    this.#ptyId = null
    const code = event.code === null ? '' : ` (code ${event.code})`
    this.term.write(`\r\n\x1b[2m[Processus terminé${code}]\x1b[0m\r\n`)
  }

  #input(data: string): void {
    const id = this.#ptyId
    if (!id || this.status !== 'running') return
    ptyWrite(id, data).catch((e: unknown) => app.fail(e))
  }

  #scheduleResize(cols: number, rows: number): void {
    if (this.#resizeTimer) clearTimeout(this.#resizeTimer)
    this.#resizeTimer = setTimeout(() => {
      this.#resizeTimer = null
      this.#sendSize(cols, rows)
    }, RESIZE_DELAY_MS)
  }

  #sendSize(cols: number, rows: number): void {
    const id = this.#ptyId
    if (!id || this.status !== 'running') return
    // A shell that already ended has no size left to change.
    ptyResize(id, cols, rows).catch(() => {})
  }
}

class TerminalStore {
  #sessions = new SvelteMap<string, TerminalSession[]>()
  #active = new SvelteMap<string, string>()
  #seeded = new Set<string>()

  list(projectId: string): TerminalSession[] {
    return this.#sessions.get(projectId) ?? []
  }

  activeId(projectId: string): string | null {
    return this.#active.get(projectId) ?? null
  }

  /** Opens the first terminal of a project, once per application run. */
  seed(projectId: string, sessionId: string | null): void {
    if (this.#seeded.has(projectId)) return
    this.#seeded.add(projectId)
    this.create(projectId, sessionId)
  }

  create(projectId: string, sessionId: string | null): TerminalSession {
    const session = new TerminalSession(projectId, sessionId)
    this.#sessions.set(projectId, [...this.list(projectId), session])
    this.#active.set(projectId, session.id)
    return session
  }

  activate(projectId: string, id: string): void {
    this.#active.set(projectId, id)
  }

  /** Kills and forgets a terminal. The neighbour becomes the active one. */
  close(projectId: string, id: string): void {
    const current = this.list(projectId)
    const index = current.findIndex((s) => s.id === id)
    if (index < 0) return
    const closed = current[index]
    const remaining = current.filter((s) => s.id !== id)
    this.#sessions.set(projectId, remaining)
    if (this.activeId(projectId) === id) {
      const next = remaining[Math.min(index, remaining.length - 1)]
      if (next) this.#active.set(projectId, next.id)
      else this.#active.delete(projectId)
    }
    closed.dispose()
  }

  /** Applies the current light or dark colours to every open terminal. */
  refreshTheme(): void {
    for (const sessions of this.#sessions.values()) {
      for (const session of sessions) {
        session.term.options.theme = terminalTheme()
        session.term.options.fontFamily = terminalFont()
      }
    }
  }
}

export const terminals = new TerminalStore()
