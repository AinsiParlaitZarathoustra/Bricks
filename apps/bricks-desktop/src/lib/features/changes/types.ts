import type { FileDiff } from '$lib/ipc/client'

/** A patch the file row loads when it is expanded. */
export interface PatchSource {
  /** Stable within the row, e.g. the journal record id. */
  key: string
  /** Heading shown above the patch. */
  title: string
  load: () => Promise<FileDiff>
}
