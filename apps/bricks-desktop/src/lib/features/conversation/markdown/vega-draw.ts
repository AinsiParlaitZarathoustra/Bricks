// The drawing of a prepared Vega-Lite specification as an SVG string, in a Web
// Worker (vega.worker.ts) or, where none exists, in the page (tests). Nothing is
// loaded: the loader refuses every load; expressions run through Vega's AST
// interpreter, never `eval` or `Function`. The view is always finalized. The
// SVG is sanitized by the caller, on the page.
import { Error as LogError, View, loader as vegaLoader, parse } from 'vega'
import { compile } from 'vega-lite'
import { expressionInterpreter } from 'vega-interpreter'

/** A specification Vega-Lite could not compile. */
export class SpecError extends Error {}

export async function drawVegaLite(spec: object): Promise<string> {
  let compiled: ReturnType<typeof compile>
  try {
    compiled = compile(spec as Parameters<typeof compile>[0])
  } catch (e) {
    throw new SpecError(e instanceof Error ? e.message : String(e))
  }
  // Nothing is ever loaded: data are inline, images and links are not fetched.
  const loader = vegaLoader()
  const refuse = async () => {
    throw new Error('les ressources externes ne sont pas chargées')
  }
  loader.load = refuse
  loader.sanitize = refuse
  const view = new View(parse(compiled.spec, undefined, { ast: true }), {
    renderer: 'none',
    loader,
    expr: expressionInterpreter,
    logLevel: LogError,
  })
  try {
    await view.runAsync()
    return await view.toSVG()
  } finally {
    view.finalize()
  }
}
