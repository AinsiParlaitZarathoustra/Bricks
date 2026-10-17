// Pure logic of the `/` palette: which commands match a draft, how a name is
// completed, how the highlight moves. No runes and no IPC, so it is unit-tested.

import type { SlashCommandInfo } from '$lib/ipc/bindings/SlashCommandInfo'

/**
 * The name typed after `/` while the draft is still a bare command name (no
 * space, no line break), or `null` when the palette does not apply.
 */
export function paletteQuery(text: string): string | null {
  if (!text.startsWith('/')) return null
  const name = text.slice(1)
  return /\s/.test(name) ? null : name
}

/**
 * Commands matching the typed name, best first: an exact name or alias, then a
 * name prefix, then an alias prefix. The registry order is kept within a rank.
 * An empty name lists every command.
 */
export function filterCommands(
  commands: readonly SlashCommandInfo[],
  query: string,
): SlashCommandInfo[] {
  const q = query.toLowerCase()
  if (q === '') return [...commands]
  const ranked: { cmd: SlashCommandInfo; index: number; rank: number }[] = []
  commands.forEach((cmd, index) => {
    const names = [cmd.name, ...cmd.aliases].map((n) => n.toLowerCase())
    let rank = 3
    if (names.includes(q)) rank = 0
    else if (cmd.name.toLowerCase().startsWith(q)) rank = 1
    else if (names.some((n) => n.startsWith(q))) rank = 2
    if (rank < 3) ranked.push({ cmd, index, rank })
  })
  ranked.sort((a, b) => a.rank - b.rank || a.index - b.index)
  return ranked.map((r) => r.cmd)
}

/** True when the typed name is exactly the command's name or one of its aliases. */
export function isExactName(query: string, cmd: SlashCommandInfo): boolean {
  const q = query.toLowerCase()
  return q === cmd.name.toLowerCase() || cmd.aliases.some((a) => a.toLowerCase() === q)
}

/** The draft after completing the name: `/name`, plus a space when the command takes arguments. */
export function completeLine(cmd: SlashCommandInfo): string {
  return cmd.args.trim() === '' ? `/${cmd.name}` : `/${cmd.name} `
}

/** Usage line of a command, e.g. `/skill <name> <request>`. */
export function usageLine(cmd: SlashCommandInfo): string {
  return cmd.args.trim() === '' ? `/${cmd.name}` : `/${cmd.name} ${cmd.args}`
}

/** Moves a highlight by `delta`, wrapping around; 0 when there is nothing to highlight. */
export function moveIndex(index: number, delta: number, count: number): number {
  if (count <= 0) return 0
  return (((index + delta) % count) + count) % count
}
