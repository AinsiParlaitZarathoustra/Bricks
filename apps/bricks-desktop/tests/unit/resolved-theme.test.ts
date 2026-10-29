// @vitest-environment happy-dom
import { afterEach, describe, expect, it } from 'vitest'
import { terminalFont, terminalFontSize, terminalTheme } from '../../src/lib/features/terminal/terminal-theme'
import { readThemeTokens } from '../../src/lib/theme/resolved-theme'

afterEach(() => document.documentElement.removeAttribute('style'))

describe('library themes use the effective CSS variables', () => {
  it('reads custom colours afresh, including a reset to the next theme', () => {
    const root = document.documentElement
    root.style.setProperty('--primary', '#a4512c')
    root.style.setProperty('--code-bg', '#fbfaf7')
    expect(terminalTheme()).toMatchObject({ cursor: '#a4512c', background: '#fbfaf7' })
    root.style.setProperty('--primary', '#e0936b')
    root.style.setProperty('--code-bg', '#14171c')
    expect(terminalTheme()).toMatchObject({ cursor: '#e0936b', background: '#14171c' })
    root.style.removeProperty('--primary')
    expect(terminalTheme()).not.toHaveProperty('cursor')
  })

  it('uses the resolved mono font and preserves the fallback without a value', () => {
    expect(terminalFont()).toBe('monospace')
    document.documentElement.style.setProperty('--code-font-family', '"Google Sans Code", monospace')
    expect(terminalFont()).toBe('"Google Sans Code", monospace')
  })

  it('uses the shared code size for xterm without changing its default', () => {
    expect(terminalFontSize()).toBe(13)
    document.documentElement.style.setProperty('--code-font-size', '15px')
    expect(terminalFontSize()).toBe(15)
  })

  it('omits missing values and colours an adapter cannot resolve', () => {
    document.documentElement.style.setProperty('--primary', '#a4512c')
    const mapping = { accent: '--primary', missing: '--missing' }
    expect(readThemeTokens(mapping)).toEqual({ accent: '#a4512c' })
    expect(readThemeTokens(mapping, () => null)).toEqual({})
  })
})
