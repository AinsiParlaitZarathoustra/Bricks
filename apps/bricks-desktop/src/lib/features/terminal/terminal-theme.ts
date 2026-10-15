// Terminal colours and font, read from the CSS tokens of src/app.css so the
// terminal follows light and dark mode. The ANSI palette maps onto the
// semantic tokens: there is no hard-coded colour here.

import type { ITheme } from '@xterm/xterm'

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

function tokenValue(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim()
}

/** xterm theme for the current light or dark mode. Read after `.dark` is set. */
export function terminalTheme(): ITheme {
  const theme: Record<string, string> = {}
  for (const [key, token] of Object.entries(COLOUR_TOKENS)) {
    const value = tokenValue(token)
    if (value) theme[key] = value
  }
  return theme as ITheme
}

export function terminalFont(): string {
  return tokenValue('--font-mono') || 'monospace'
}
