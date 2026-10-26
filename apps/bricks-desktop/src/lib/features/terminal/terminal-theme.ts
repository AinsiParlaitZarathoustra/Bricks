// Terminal colours and font, read from the CSS tokens of src/app.css so the
// terminal follows light and dark mode. The ANSI palette maps onto the
// semantic tokens: there is no hard-coded colour here.

import type { ITheme } from '@xterm/xterm'
import { readThemeToken, readThemeTokens } from '$lib/theme/resolved-theme'
import tokens from '$lib/theme/tokens.json'

const COLOUR_TOKENS: Record<string, string> = {
  background: '--code-bg',
  foreground: '--code-foreground',
  cursor: '--primary',
  cursorAccent: '--code-bg',
  selectionBackground: '--muted',
  black: '--muted-foreground',
  red: '--destructive',
  green: '--success',
  yellow: '--warning',
  blue: '--info',
  magenta: '--primary',
  cyan: '--info',
  white: '--foreground',
  brightBlack: '--muted-foreground',
  brightRed: '--destructive',
  brightGreen: '--success',
  brightYellow: '--warning',
  brightBlue: '--info',
  brightMagenta: '--primary',
  brightCyan: '--info',
  brightWhite: '--card-foreground',
}

/** xterm theme for the current light or dark mode. Read after `.dark` is set. */
export function terminalTheme(): ITheme {
  return readThemeTokens(COLOUR_TOKENS) as ITheme
}

export function terminalFont(): string {
  return readThemeToken('--code-font-family') || 'monospace'
}

/** xterm expects a size in CSS pixels; the shared code size uses that unit. */
export function terminalFontSize(): number {
  return Number.parseFloat(readThemeToken('--code-font-size') || tokens.fontSizes.code)
}
