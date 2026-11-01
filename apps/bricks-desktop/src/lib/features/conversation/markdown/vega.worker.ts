// Draws Vega-Lite charts off the page: a chart that computes for too long is
// stopped with its worker (vega.ts), the app never freezes.
import { SpecError, drawVegaLite } from './vega-draw'

self.onmessage = async (event: MessageEvent<{ id: number; spec: object }>) => {
  const { id, spec } = event.data
  try {
    self.postMessage({ id, svg: await drawVegaLite(spec) })
  } catch (e) {
    self.postMessage({ id, error: e instanceof Error ? e.message : String(e), spec: e instanceof SpecError })
  }
}
