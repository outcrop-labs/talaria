// Host-side Vite plugin for independently-compiled apps:
//   1. extra client/SSR entries `runtime/rt-*.js` so a separately-built app
//      chunk imports the host's Svelte/SDK, not a second copy
//   2. `/@app/<slug>/…` resolves to the app's source (dev HMR)
//   3. `/app-builds/` in dev, through the production door (app-builds-door.ts)
import { existsSync, mkdirSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import type { Plugin, ViteDevServer } from 'vite'
import { clientSpecifiers, hostBuildStamp, serverSpecifiers, shimFileName } from './specifiers'
import { appsDir, slugOk } from '../server/app-build/paths'

const VIRTUAL = '\0talaria-rt:'

export function appRuntimeHost(opts: { ssr: boolean }): Plugin {
  const specs = opts.ssr ? serverSpecifiers() : clientSpecifiers()
  return {
    name: 'talaria-app-runtime-host',
    enforce: 'pre',
    options(inputOpts) {
      const prev = inputOpts.input
      const obj: Record<string, string> =
        typeof prev === 'string'
          ? { main: prev }
          : Array.isArray(prev)
            ? Object.fromEntries(prev.map((p, i) => [`entry${i}`, p]))
            : { ...(prev as Record<string, string> | undefined) }
      for (const spec of specs) obj[`runtime/${shimFileName(spec)}`] = VIRTUAL + spec
      inputOpts.input = obj
      inputOpts.preserveEntrySignatures = 'strict'
      return inputOpts
    },
    resolveId(id) {
      if (id.startsWith(VIRTUAL)) return id
      return null
    },
    load(id) {
      if (!id.startsWith(VIRTUAL)) return null
      const spec = id.slice(VIRTUAL.length)
      return `export * from ${JSON.stringify(spec)};\nimport ${JSON.stringify(spec)};\n`
    },
    closeBundle() {
      if (opts.ssr) return
      const dir = join(process.cwd(), 'dist/client/runtime')
      mkdirSync(dir, { recursive: true })
      writeFileSync(join(dir, 'build-id'), `${hostBuildStamp()}\n`)
    },
  }
}

export function appRuntimeDev(): Plugin {
  return {
    name: 'talaria-app-runtime-dev',
    resolveId(id) {
      const m = /^\/@app\/([a-z0-9][a-z0-9-]{0,63})\/(.+)$/.exec(id)
      if (!m?.[1] || !m[2]) return null
      const abs = join(appsDir(), m[1], m[2])
      return existsSync(abs) ? abs : null
    },

    configureServer(server: ViteDevServer) {
      void server
        .ssrLoadModule('/src/server/app-load.ts')
        .then((mod: { setSsrLoader?: (fn: (abs: string) => Promise<unknown>) => void; setSsrInvalidator?: (fn: (dir: string) => void) => void }) => {
          mod.setSsrLoader?.((abs) => server.ssrLoadModule(`/@fs${abs}`))
          // An app update swaps apps/<slug> in one rename: drop every module
          // either graph holds from under the old tree, so the next load (and
          // the next page load of /@app/<slug>/…) reads the new code.
          mod.setSsrInvalidator?.((dir) => {
            const prefix = dir.endsWith('/') ? dir : `${dir}/`
            for (const env of Object.values(server.environments)) {
              const graph = env.moduleGraph
              for (const m of graph.idToModuleMap.values()) {
                if (m.file?.startsWith(prefix)) graph.invalidateModule(m)
              }
            }
          })
        })
      // /app-builds/ in dev answers through the same door as production
      // (server/app-builds-door.ts): app.js, app.css and assets/** only, to a
      // session granted the app. Surfaces load from /@app/ source in dev, so
      // this path matters only for a dev box exercising built apps.
      server.middlewares.use((req, res, next) => {
        if (!(req.url ?? '/').startsWith('/app-builds/')) return next()
        void (async () => {
          const mod = (await server.ssrLoadModule('/src/server/app-builds-door.ts')) as {
            serveAppBuild: (request: Request) => Promise<Response>
          }
          const headers = new Headers()
          if (req.headers.cookie) headers.set('cookie', req.headers.cookie)
          const url = new URL(req.url ?? '/', `http://${req.headers.host ?? 'localhost'}`)
          const response = await mod.serveAppBuild(new Request(url, { method: req.method, headers }))
          res.writeHead(response.status, Object.fromEntries(response.headers.entries()))
          res.end(Buffer.from(await response.arrayBuffer()))
        })().catch((err: unknown) => {
          console.error(`[app-builds-dev] ${req.method} ${req.url}`, err)
          if (!res.headersSent) res.writeHead(500, { 'content-type': 'text/plain' })
          res.end('Internal Server Error')
        })
      })
    },
  }
}

export { slugOk }
