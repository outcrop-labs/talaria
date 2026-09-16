// Host-side Vite plugin for independently-compiled apps:
//   1. extra client/SSR entries `runtime/rt-*.js` so a separately-built app
//      chunk imports the host's Svelte/SDK, not a second copy
//   2. `/@app/<slug>/…` resolves to the app's source (dev HMR)
//   3. `/app-builds/` served from the artifact dir in dev
import { existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { join, sep } from 'node:path'
import type { Plugin, ViteDevServer } from 'vite'
import { clientSpecifiers, hostBuildStamp, serverSpecifiers, shimFileName } from './specifiers'
import { appBuildsDir, appsDir, slugOk } from '../server/app-build/paths'

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
      void server.ssrLoadModule('/src/server/app-load.ts').then((mod: { setSsrLoader?: (fn: (abs: string) => Promise<unknown>) => void }) => {
        mod.setSsrLoader?.((abs) => server.ssrLoadModule(`/@fs${abs}`))
      })
      server.middlewares.use((req, res, next) => {
        const pathname = decodeURIComponent((req.url ?? '/').split('?')[0] ?? '/')
        if (!pathname.startsWith('/app-builds/')) return next()
        if (pathname.includes('..')) {
          res.statusCode = 400
          res.end('bad path')
          return
        }
        const rel = pathname.slice('/app-builds/'.length)
        const root = appBuildsDir()
        const file = join(root, rel)
        if (!file.startsWith(root + sep) && file !== root) {
          res.statusCode = 400
          res.end('bad path')
          return
        }
        if (!existsSync(file)) {
          res.statusCode = 404
          res.end('not found')
          return
        }
        const data = readFileSync(file)
        const type = file.endsWith('.js')
          ? 'application/javascript'
          : file.endsWith('.css')
            ? 'text/css'
            : file.endsWith('.json')
              ? 'application/json'
              : 'application/octet-stream'
        res.setHeader('Content-Type', type)
        res.setHeader('Cache-Control', 'public, max-age=31536000, immutable')
        res.end(data)
      })
    },
  }
}

export { slugOk }
