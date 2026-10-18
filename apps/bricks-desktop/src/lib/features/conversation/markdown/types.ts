// Contracts of the assistant-message Markdown pipeline.
//
//   source (kept whole) → marked tokens + Bricks extensions + document dependencies
//     → blocks keyed by their offset: frozen / to reconcile / open
//     → bounded per-message cache of rendered blocks (highlighting included)
//     → Svelte blocks: sanitized HTML fragments, or dedicated components
//
// Owners: document/extensions/render/clicks — logic 1; sanitize/math/mermaid/
// file references/diff adapter — logic 2; components and styles — Opus.

/** One rendered top-level block of a message. `key` is stable while the block keeps its start offset. */
export type RenderedBlock =
  | { key: string; kind: 'html'; html: string }
  /** A top-level ```mermaid fence. `complete` is false while the fence is still open (streaming). */
  | { key: string; kind: 'mermaid'; source: string; complete: boolean }
  /** A top-level ```diff / ```patch fence: a proposal shown read-only, never an applied change. */
  | { key: string; kind: 'diff'; source: string; complete: boolean }

/** Counters of one `update`, for tests and measurements. */
export interface RenderStats {
  /** Characters given to the block lexer in this update (tail + re-inlined blocks). */
  lexedChars: number
  /** Blocks rendered again in this update. */
  renderedBlocks: number
  /** Blocks served from the cache in this update. */
  reusedBlocks: number
  /** highlight.js calls in this update. */
  highlightCalls: number
  /** Math expressions rendered by the math renderer in this update. */
  mathRenders: number
  /** Frozen blocks invalidated by a dependency change (link/footnote definitions, math renderer). */
  invalidatedBlocks: number
  /** True when the update re-lexed the whole source (final reconciliation or replaced source). */
  fullLex: boolean
  /** True when the message could not be rendered: its source is shown as one escaped paragraph. */
  fallback: boolean
  /** Wall time of the update, ms. */
  ms: number
}

/** Math renderer installed once KaTeX is loaded. Returns sanitized HTML, or an error message (shown as text). */
export type MathRenderer = (
  tex: string,
  display: boolean,
  /** Per-message macro table, owned by the document; never shared between messages. */
  macros: Record<string, string>,
) => { html: string } | { error: string }

export interface MarkdownDocumentOptions {
  /** Unique per message: prefixes heading ids, footnote ids and Mermaid ids. Characters [a-z0-9-]. */
  idPrefix: string
  /** Maximum cached blocks (bounded cache). */
  cacheLimit?: number
}

/** A file reference found in a message: a path relative to the conversation's project. */
export interface FileReference {
  path: string
  /** 1-based first line, when given. */
  line?: number
  /** 1-based last line of a range (≥ line), when given. */
  endLine?: number
}

/** Read-only window of a project file (IPC `project_file_read`): generated binding, same names. */
export type { ProjectFileWindow } from '../../../ipc/bindings/ProjectFileWindow'

/** A heading of a message, in document order: `id` is the HTML id, `text` plain text, `level` the Markdown level (1–6). */
export interface MarkdownHeading {
  id: string
  text: string
  level: number
}
