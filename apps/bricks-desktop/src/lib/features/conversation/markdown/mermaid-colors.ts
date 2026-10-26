// Theme colours for Mermaid, read from the CSS tokens of the current mode.
// The tokens are oklch(); Mermaid only parses sRGB, so each one is resolved
// to hex through a 1×1 canvas. Read after the `.dark` class is set.

import { cssColorToHex, readThemeTokens } from '$lib/theme/resolved-theme'

/** Keys of `mermaid.ts` (`safeColors`) → CSS tokens of the app. */
const TOKENS: Record<string, string> = {
  background: '--code-bg',
  foreground: '--foreground',
  card: '--card',
  border: '--border',
  primary: '--primary',
  muted: '--muted-foreground',
  chart1: '--chart-1',
  chart2: '--chart-2',
  chart3: '--chart-3',
  chart4: '--chart-4',
  chart5: '--chart-5',
}

export function mermaidColors(): Record<string, string> {
  return readThemeTokens(TOKENS, cssColorToHex)
}
