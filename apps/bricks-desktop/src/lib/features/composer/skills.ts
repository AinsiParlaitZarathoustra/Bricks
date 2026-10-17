// Skills chosen in the composer for the next message (`Prompt.skills`). The engine
// decides what is usable; these helpers only read its listing. Pure: unit-tested
// (tests/unit/composer-skills.test.ts).
import type { SkillListing, SkillsListing } from '$lib/ipc/engine'

export interface SkillChoice {
  name: string
  description: string | null
  /** Already active in the conversation's agent. */
  active: boolean
}

/** Engine states that can be chosen for a message: `available` (wins its name) and `active`. */
const USABLE_STATES = new Set(['available', 'active'])

/** The skills a message can use now, one per name, in the engine's order. */
export function usableSkills(entries: SkillListing[]): SkillChoice[] {
  const names = new Set<string>()
  const out: SkillChoice[] = []
  for (const entry of entries) {
    if (!USABLE_STATES.has(entry.state) || names.has(entry.name)) continue
    names.add(entry.name)
    out.push({ name: entry.name, description: entry.description ?? null, active: entry.state === 'active' })
  }
  return out
}

/** Adds the name when absent, removes it when present. */
export function toggleSkill(selected: string[], name: string): string[] {
  return selected.includes(name) ? selected.filter((n) => n !== name) : [...selected, name]
}

/** The selection once a run has started: the skills sent with it are no longer chosen. */
export function selectionAfterRun(selected: string[], sent: string[]): string[] {
  return selected.filter((n) => !sent.includes(n))
}

/** True when project skills wait for the project trust decision (they are not offered). */
export function hasProjectSkillsAwaitingApproval(listing: SkillsListing): boolean {
  return listing.entries.some((entry) => entry.state === 'pending_approval')
}
