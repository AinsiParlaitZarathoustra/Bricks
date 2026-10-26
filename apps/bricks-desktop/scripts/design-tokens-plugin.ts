import path from 'node:path'
import type { Plugin, ResolvedConfig, ViteDevServer } from 'vite'
import { generateTokens } from './generate-tokens.mjs'

type GenerateTokens = typeof generateTokens

/** Keep dev alive on incomplete JSON saves, while builds fail on invalid token sources. */
export function designTokensPlugin(
  sourceFile = path.resolve('./src/lib/theme/tokens.json'),
  generate: GenerateTokens = generateTokens,
): Plugin {
  let command: ResolvedConfig['command'] | undefined

  const reportDevError = (server: ViteDevServer, error: unknown) => {
    const detail = error instanceof Error ? error : new Error(String(error))
    server.config.logger.error(`[bricks-design-tokens] Could not generate theme CSS: ${detail.message}`, { error: detail })
  }

  const generateForDev = (server: ViteDevServer): boolean => {
    try {
      generate()
      return true
    } catch (error) {
      reportDevError(server, error)
      return false
    }
  }

  return {
    name: 'bricks-design-tokens',
    enforce: 'pre',
    configResolved(config) {
      command = config.command
    },
    buildStart() {
      if (command === 'build') generate()
    },
    configureServer(server) {
      server.watcher.add(sourceFile)
      generateForDev(server)
      server.watcher.on('change', (file) => {
        if (path.resolve(file) !== sourceFile) return
        if (generateForDev(server)) server.ws.send({ type: 'full-reload' })
      })
    },
  }
}
