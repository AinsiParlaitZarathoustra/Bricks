import mermaidPackage from 'mermaid/package.json'
import type { DiagramSettings } from '$lib/ipc/bindings/DiagramSettings'
import type { DiagramViewerSettings } from '$lib/ipc/bindings/DiagramViewerSettings'

type Config = NonNullable<Parameters<(typeof import('mermaid'))['default']['initialize']>[0]>
export const MERMAID_VERSION = mermaidPackage.version
// Verified against the installed Mermaid schema and theme implementations.
export const MERMAID_THEMES = ['default', 'forest', 'dark', 'neutral', 'base'] as const satisfies NonNullable<Config['theme']>[]
export const MERMAID_LOOKS = ['classic', 'handDrawn', 'neo'] as const satisfies NonNullable<Config['look']>[]
export const MERMAID_SECURITY_LEVELS = ['sandbox', 'strict', 'antiscript', 'loose'] as const satisfies NonNullable<Config['securityLevel']>[]
export type MermaidTheme = (typeof MERMAID_THEMES)[number]
export type MermaidLook = (typeof MERMAID_LOOKS)[number]
export type MermaidSecurityLevel = (typeof MERMAID_SECURITY_LEVELS)[number]

export function defaultDiagramSettings(): DiagramSettings {
  return {
    mermaid: { securityLevel: 'sandbox', theme: 'forest', look: 'classic' },
    diagramViewer: {
      followAppTheme: true, allowZoom: true, allowPan: true, allowFullscreen: true,
      allowDownload: true, allowCopy: true, allowTooltips: true, allowLinks: true,
      allowSelection: false, allowAnimations: true, allowEdit: false, allowJavaScript: false,
      capabilitiesRevision: 1,
    },
  }
}

/** Old files get defaults; script and editing remain locked even before Rust responds. */
export function diagramSettings(value?: DiagramSettings): DiagramSettings {
  const defaults = defaultDiagramSettings()
  const mermaid = value?.mermaid
  const viewer = value?.diagramViewer
  const safeViewer = { ...defaults.diagramViewer }
  for (const key of Object.keys(safeViewer) as (keyof DiagramViewerSettings)[]) {
    const value = viewer?.[key]
    if (typeof value === typeof safeViewer[key]) (safeViewer as Record<string, unknown>)[key] = value
  }
  return {
    mermaid: {
      securityLevel: mermaid && MERMAID_SECURITY_LEVELS.includes(mermaid.securityLevel) ? mermaid.securityLevel : defaults.mermaid.securityLevel,
      theme: mermaid && MERMAID_THEMES.includes(mermaid.theme) ? mermaid.theme : defaults.mermaid.theme,
      look: mermaid && MERMAID_LOOKS.includes(mermaid.look) ? mermaid.look : defaults.mermaid.look,
    },
    diagramViewer: { ...safeViewer, allowEdit: false, allowJavaScript: false },
  }
}

/** `app` while the diagrams follow Bricks's theme, otherwise the chosen Mermaid theme. */
export function diagramTheme(settings: DiagramSettings): 'app' | MermaidTheme {
  return settings.diagramViewer.followAppTheme ? 'app' : settings.mermaid.theme
}

/** The background mode of a diagram: the app's mode, or the one the Mermaid theme is drawn for. */
export function diagramMode(settings: DiagramSettings, appMode: 'light' | 'dark'): 'light' | 'dark' {
  const theme = diagramTheme(settings)
  return theme === 'app' ? appMode : theme === 'dark' ? 'dark' : 'light'
}
