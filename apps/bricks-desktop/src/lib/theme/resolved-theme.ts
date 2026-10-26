// The effective CSS theme, including user colour overrides. Library adapters
// keep their own role mappings, but share the same DOM reads and conversion.

export function readThemeToken(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim()
}

export function readThemeTokens(
  mapping: Readonly<Record<string, string>>,
  resolve: (value: string) => string | null = (value) => value,
): Record<string, string> {
  const style = getComputedStyle(document.documentElement)
  const values: Record<string, string> = {}
  for (const [role, token] of Object.entries(mapping)) {
    const value = resolve(style.getPropertyValue(token).trim())
    if (value) values[role] = value
  }
  return values
}

let colorContext: CanvasRenderingContext2D | null | undefined

/** Resolve CSS colours (including OKLCH) to the sRGB format Mermaid accepts. */
export function cssColorToHex(color: string): string | null {
  if (colorContext === undefined) {
    const canvas = document.createElement('canvas')
    canvas.width = canvas.height = 1
    colorContext = canvas.getContext('2d', { willReadFrequently: true })
  }
  if (!colorContext || !color) return null
  colorContext.clearRect(0, 0, 1, 1)
  colorContext.fillStyle = '#000'
  colorContext.fillStyle = color
  colorContext.fillRect(0, 0, 1, 1)
  const [r, g, b] = colorContext.getImageData(0, 0, 1, 1).data
  return '#' + [r, g, b].map((value) => value.toString(16).padStart(2, '0')).join('')
}
