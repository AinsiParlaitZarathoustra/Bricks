// The four suggestions of the welcome screen, and what picking one does to the draft:
// it fills the composer, it never sends.

import type { Component } from 'svelte'
import Search from '@lucide/svelte/icons/search'
import Sparkles from '@lucide/svelte/icons/sparkles'
import GitCompare from '@lucide/svelte/icons/git-compare'
import FlaskConical from '@lucide/svelte/icons/flask-conical'
import { t } from '$lib/i18n/i18n.svelte'
import type { MessageKey } from '$lib/i18n/messages/en'

export interface Starter {
  id: string
  label: MessageKey
  prompt: MessageKey
  icon: Component<{ class?: string; 'aria-hidden'?: boolean | 'true' | 'false' }>
}

export const STARTERS: readonly Starter[] = [
  { id: 'analyze', label: 'welcome.analyze', prompt: 'welcome.analyzePrompt', icon: Search },
  { id: 'feature', label: 'welcome.feature', prompt: 'welcome.featurePrompt', icon: Sparkles },
  { id: 'review', label: 'welcome.review', prompt: 'welcome.reviewPrompt', icon: GitCompare },
  { id: 'tests', label: 'welcome.tests', prompt: 'welcome.testsPrompt', icon: FlaskConical },
]

/** The text a starter puts in the composer, in the interface language. */
export function starterText(starter: Starter): string {
  return t(starter.prompt)
}

/**
 * Puts `text` in the draft. Returns the text it replaced when the draft held one
 * (so it can be given back), or `null` when the draft was empty.
 */
export function prefillDraft(draft: { text: string }, text: string): string | null {
  const previous = draft.text
  draft.text = text
  return previous.trim() ? previous : null
}
