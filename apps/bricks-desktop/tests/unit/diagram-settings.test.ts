import { describe, expect, it } from 'vitest'
import { defaultDiagramSettings, diagramMode, diagramSettings, diagramTheme, MERMAID_SECURITY_LEVELS, MERMAID_VERSION } from '../../src/lib/features/diagrams/settings'

describe('Bricks diagram settings', () => {
  it('follows the app theme by default, and a chosen Mermaid theme replaces it', () => {
    const prefs = defaultDiagramSettings()
    expect(MERMAID_VERSION).toBe('12.1.0')
    expect(prefs.mermaid).toEqual({ securityLevel: 'sandbox', theme: 'forest', look: 'classic' })
    expect(prefs.diagramViewer.followAppTheme).toBe(true)
    expect(diagramTheme(prefs)).toBe('app')
    expect(diagramMode(prefs, 'dark')).toBe('dark')
    expect(diagramMode(prefs, 'light')).toBe('light')
    prefs.diagramViewer.followAppTheme = false
    expect(diagramTheme(prefs)).toBe('forest')
    expect(diagramMode(prefs, 'dark')).toBe('light')
    prefs.mermaid.theme = 'dark'
    expect(diagramMode(prefs, 'light')).toBe('dark')
  })

  it('offers the four Mermaid security levels, sandbox first and by default', () => {
    expect(MERMAID_SECURITY_LEVELS).toEqual(['sandbox', 'strict', 'antiscript', 'loose'])
    for (const level of MERMAID_SECURITY_LEVELS) {
      const prefs = defaultDiagramSettings()
      prefs.mermaid.securityLevel = level
      expect(diagramSettings(prefs).mermaid.securityLevel).toBe(level)
    }
    const invented = defaultDiagramSettings()
    ;(invented.mermaid as { securityLevel: string }).securityLevel = 'invented'
    expect(diagramSettings(invented).mermaid.securityLevel).toBe('sandbox')
  })

  it('locks scripts and editing, keeps the links and animations chosen, and supplies defaults to old preferences', () => {
    expect(diagramSettings()).toEqual(defaultDiagramSettings())
    expect(defaultDiagramSettings().diagramViewer).toMatchObject({ allowLinks: true, allowAnimations: true })
    const prefs = defaultDiagramSettings()
    Object.assign(prefs.diagramViewer, { allowLinks: false, allowJavaScript: true, allowEdit: true, allowAnimations: false, allowZoom: false })
    expect(diagramSettings(prefs)).toMatchObject({ diagramViewer: { allowLinks: false, allowJavaScript: false, allowEdit: false, allowAnimations: false, allowZoom: false } })
  })
})
