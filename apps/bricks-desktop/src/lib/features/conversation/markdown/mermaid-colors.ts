// Theme colours for Mermaid, read from the CSS tokens of the current mode.
// The tokens are oklch(); Mermaid only parses sRGB, so each one is resolved
// to hex through a 1×1 canvas. Read after the `.dark` class is set.

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

let context: CanvasRenderingContext2D | null | undefined

function toHex(color: string): string | null {
  if (context === undefined) {
    const canvas = document.createElement('canvas')
    canvas.width = canvas.height = 1
    context = canvas.getContext('2d', { willReadFrequently: true })
  }
  if (!context || !color) return null
  context.clearRect(0, 0, 1, 1)
  context.fillStyle = '#000'
  context.fillStyle = color
  context.fillRect(0, 0, 1, 1)
  const [r, g, b] = context.getImageData(0, 0, 1, 1).data
  return '#' + [r, g, b].map((v) => v.toString(16).padStart(2, '0')).join('')
}

export function mermaidColors(): Record<string, string> {
  const style = getComputedStyle(document.documentElement)
  const out: Record<string, string> = {}
  for (const [key, token] of Object.entries(TOKENS)) {
    const hex = toHex(style.getPropertyValue(token).trim())
    if (hex) out[key] = hex
  }
  return out
}
