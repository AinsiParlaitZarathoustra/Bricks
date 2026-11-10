// The welcome screen's field of characters: pure maths, no DOM, so it is tested in node.
// A slow sine "wind" drives a value per cell; the value is hollowed out around the centre
// (the welcome sits there) and ordered-dithered into a short ramp of glyphs.

/** The ramp, from empty to dense. */
export const GLYPHS = ' .·:-=+*#'

/** Side of a cell, in CSS pixels. */
export const CELL = 12

/** Time between two frames, in ms: a deliberately stepped motion (~12 frames a second). */
export const FRAME_MS = 80

/** 4 × 4 ordered-dithering thresholds (Bayer), 0 to 15. */
export const BAYER: readonly (readonly number[])[] = [
  [0, 8, 2, 10],
  [12, 4, 14, 6],
  [3, 11, 1, 9],
  [15, 7, 13, 5],
]

const clamp01 = (x: number) => Math.min(1, Math.max(0, x))

/** The wind at a cell and a time: a mean of three sines, in [-1, 1]. */
export function noise(x: number, y: number, t: number): number {
  const wx = t * 0.012
  const wy = t * 0.003
  const phase = t * 0.0004
  return (
    (Math.sin(x * 0.11 + wx + phase) +
      Math.sin(y * 0.17 - wy + x * 0.05) +
      Math.sin((x + y) * 0.07 + phase * 3)) /
    3
  )
}

/** How much a cell at normalised distance `r` from the centre (0 centre, 1 corner) may show. */
export function hollow(r: number): number {
  return clamp01((r - 0.34) / 0.5)
}

/** The glyph of a cell, or `null` when the cell stays empty. */
export function glyphAt(gx: number, gy: number, r: number, t: number): string | null {
  const v = ((noise(gx, gy, t) + 1) / 2) * hollow(r)
  if (v * 16 <= BAYER[gy % 4][gx % 4] * 0.6) return null
  const glyph = GLYPHS[Math.min(GLYPHS.length - 1, Math.floor(v * v * GLYPHS.length))]
  return glyph === ' ' ? null : glyph
}

/** Draws one frame of the field on a 2D context of `width` × `height` CSS pixels. */
export function drawField(
  ctx: CanvasRenderingContext2D,
  width: number,
  height: number,
  t: number,
  ink: string,
): void {
  ctx.clearRect(0, 0, width, height)
  ctx.fillStyle = ink
  ctx.font = '11px ui-monospace, SFMono-Regular, Menlo, monospace'
  ctx.textBaseline = 'top'
  const cols = Math.ceil(width / CELL)
  const rows = Math.ceil(height / CELL)
  const cx = width / 2
  const cy = height * 0.45
  const half = Math.hypot(width / 2, height / 2) || 1
  for (let gy = 0; gy < rows; gy++) {
    for (let gx = 0; gx < cols; gx++) {
      const px = gx * CELL
      const py = gy * CELL
      const glyph = glyphAt(gx, gy, Math.hypot(px - cx, py - cy) / half, t)
      if (glyph) ctx.fillText(glyph, px, py)
    }
  }
}
