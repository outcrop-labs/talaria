// One app = one standalone Vite build. Client chunk + CSS + optional
// server/mcp chunks, every shared specifier external and rewritten to the
// host's /runtime/rt-*.js entries. A bundled svelte is a build error
// (`effect_orphan` at runtime), not a surprise later.
//
// Changing what this file emits, or how the host reads it? Bump BUILD_RECIPE
// in ./key.ts in the same change, so builds made by the old recipe are
// rebuilt instead of reused.
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'
import { build as viteBuild, type Plugin } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'
import tailwindcss from '@tailwindcss/vite'
import {
  bundledShared,
  clientRuntimePath,
  hostBuildStamp,
  isClientSpecifier,
  isServerSpecifier,
  shimFileName,
} from '@/app-runtime/specifiers'
import { sourceKey } from './key'
import { appBuildKeyDir, appDir, clientlessError, declaredSurfacesAt, manifestError, uiRoot } from './paths'
import { pruneSupersededBuilds, readCurrent, writeCurrent, type AppBuildCurrent } from './status'

function externalsGuard(): Plugin {
  return {
    name: 'talaria-app-externals-guard',
    generateBundle(_, bundle) {
      for (const chunk of Object.values(bundle)) {
        if (chunk.type !== 'chunk') continue
        const leaked = bundledShared(chunk.moduleIds)
        if (leaked.length) {
          throw new Error(`shared specifier bundled into app chunk: ${leaked[0]}`)
        }
      }
    },
  }
}

function dropCssImport(): Plugin {
  return {
    name: 'talaria-app-drop-css-import',
    generateBundle(_, bundle) {
      for (const chunk of Object.values(bundle)) {
        if (chunk.type === 'chunk') chunk.code = chunk.code.replace(/import\s+['"]\.\/app\.css['"];?\n?/g, '')
      }
    },
  }
}

function hostId(): string {
  const stamped = join(uiRoot(), 'dist/client/runtime/build-id')
  if (existsSync(stamped)) return readFileSync(stamped, 'utf8').trim() || hostBuildStamp()
  return hostBuildStamp()
}

function sdkServerShim(): string | null {
  const p = join(uiRoot(), 'dist/server/runtime', `${shimFileName('@talaria/sdk/server')}.js`)
  return existsSync(p) ? p : null
}

async function buildClient(appDir: string, outDir: string): Promise<void> {
  const styles = join(uiRoot(), 'src/styles.css')
  const cssEntry = join(outDir, '_entry.css')
  mkdirSync(outDir, { recursive: true })
  writeFileSync(cssEntry, `@import ${JSON.stringify(styles)};\n@source ${JSON.stringify(appDir)};\n`)
  await viteBuild({
    configFile: false,
    root: appDir,
    cacheDir: join(outDir, '.cache-client'),
    logLevel: 'warn',
    plugins: [
      tailwindcss(),
      svelte({ configFile: join(uiRoot(), 'svelte.config.ts') }),
      externalsGuard(),
      dropCssImport(),
    ],
    resolve: {
      dedupe: ['svelte', '@tanstack/svelte-query', '@lucide/svelte'],
    },
    build: {
      outDir,
      emptyOutDir: false,
      cssCodeSplit: true,
      rollupOptions: {
        input: { app: join(appDir, 'app.ts'), style: cssEntry },
        // The host imports app.js and reads its default export (defineApp).
        // Vite's app-build default (false) lets Rollup drop an entry's
        // exports, which left app.js with none: the probe refused it and the
        // host had no surfaces to mount.
        preserveEntrySignatures: 'strict',
        external: (id) => isClientSpecifier(id),
        output: {
          format: 'es',
          entryFileNames: (c) => (c.name === 'app' ? 'app.js' : '_style.js'),
          assetFileNames: (a) => (typeof a.name === 'string' && a.name.endsWith('.css') ? 'app.css' : 'assets/[name][extname]'),
          paths: (id) => (isClientSpecifier(id) ? clientRuntimePath(id) : id),
        },
      },
    },
  })
  const leftover = join(outDir, '_style.js')
  if (existsSync(leftover)) rmSync(leftover)
}

/** Where an SSR build (server.ts, mcp.ts) writes into the shared build-key
 *  directory. Its entry is server.js or mcp.js; every chunk and asset it
 *  splits off goes under `_server/` or `_mcp/`. Vite's defaults would put
 *  them in `assets/`, beside the client's files, and the /app-builds/ door
 *  serves `assets/**` to anyone holding a grant on the app — so a dynamic
 *  import() in server.ts would ship that piece of the server to the browser.
 *  A leading underscore keeps the names apart from anything the client build
 *  writes, and the door serves no top-level directory but `assets/`
 *  (app-builds-door.ts). */
export function serverishFileNames(outFile: string): { entryFileNames: string; chunkFileNames: string; assetFileNames: string } {
  const mcp = outFile.endsWith('mcp.js')
  const dir = mcp ? '_mcp' : '_server'
  return {
    entryFileNames: mcp ? 'mcp.js' : 'server.js',
    chunkFileNames: `${dir}/[name]-[hash].js`,
    assetFileNames: `${dir}/[name]-[hash][extname]`,
  }
}

async function buildServerish(entry: string, outFile: string): Promise<void> {
  const shim = sdkServerShim()
  if (!shim) return
  const outDir = join(outFile, '..')
  await viteBuild({
    configFile: false,
    root: join(entry, '..'),
    cacheDir: join(outDir, '.cache-ssr'),
    logLevel: 'warn',
    plugins: [externalsGuard()],
    ssr: { noExternal: [] },
    build: {
      ssr: true,
      outDir,
      emptyOutDir: false,
      rollupOptions: {
        input: entry,
        external: (id) => isServerSpecifier(id) || id.startsWith('node:'),
        output: {
          format: 'es',
          ...serverishFileNames(outFile),
          paths: (id) => (isServerSpecifier(id) ? pathToFileURL(shim).href : id),
        },
      },
    },
  })
}

function dirBytes(dir: string): number {
  let n = 0
  if (!existsSync(dir)) return 0
  for (const name of readdirSync(dir)) {
    const p = join(dir, name)
    const st = statSync(p)
    n += st.isDirectory() ? dirBytes(p) : st.size
  }
  return n
}

function listFiles(dir: string): string[] {
  const names = ['app.js', 'app.css', 'server.js', 'mcp.js']
  return names.filter((n) => existsSync(join(dir, n)))
}

/** An app without app.ts builds only when its manifest declares no surface
 *  and it ships something else to run (`clientlessError`, the rule the load
 *  probe holds an existing build to as well); otherwise the build fails,
 *  saying which. */
function clientlessOk(dir: string, hasServerSide: boolean): void {
  const error = clientlessError(declaredSurfacesAt(dir), hasServerSide)
  if (error) throw new Error(error)
}

/** Compile one app into `<builds>/<slug>/<key>/`. Idempotent when the key matches. */
export async function buildApp(slug: string): Promise<AppBuildCurrent> {
  return buildAppAt(slug, appDir(slug), { pointer: true })
}

/** Compile the tree at `dir` as app `slug`, into `<builds>/<slug>/<key>/`.
 *
 *  `pointer: true` is the app's own build: current.json follows it
 *  (building, then ready or failed) and superseded builds are pruned.
 *  `pointer: false` is an update's staged tree (`apps/.<slug>.next`): the
 *  build lands under its key beside the live one and current.json is not
 *  touched, so the live app keeps serving while the new version is checked.
 *  The key hashes the tree's files by their path inside it, so the staged
 *  build's key is the one the tree has once it is swapped into apps/<slug>.
 *
 *  A manifest the rules refuse fails the build with the reason; a tree with
 *  no talaria.json throws, as an app that is not there. */
export async function buildAppAt(slug: string, dir: string, opts: { pointer: boolean }): Promise<AppBuildCurrent> {
  const src = join(dir, 'talaria.json')
  if (!existsSync(src)) throw new Error(`no app "${slug}"`)
  const hostBuild = hostId()
  const key = sourceKey(dir, hostBuild)
  const failed = (error: string): AppBuildCurrent => ({ key, status: 'failed', hostBuild, builtAt: Date.now(), bytes: 0, files: [], error })
  const refused = manifestError(dir, slug)
  if (refused) {
    const cur = failed(refused)
    if (opts.pointer) writeCurrent(slug, cur)
    return cur
  }
  const existing = readCurrent(slug)
  if (existing?.status === 'ready' && existing.key === key) return existing
  if (opts.pointer) writeCurrent(slug, { key, status: 'building', hostBuild, builtAt: Date.now(), bytes: 0, files: [] })
  const outDir = appBuildKeyDir(slug, key)
  mkdirSync(outDir, { recursive: true })
  try {
    const serverTs = join(dir, 'server.ts')
    const mcpTs = join(dir, 'mcp.ts')
    // The client is built only when there is one. An app whose talaria.json
    // declares no surfaces — a server- or MCP-only plugin — needs no app.ts
    // and gets no app.js; one that declares a surface must ship app.ts.
    if (existsSync(join(dir, 'app.ts'))) await buildClient(dir, outDir)
    else clientlessOk(dir, existsSync(serverTs) || existsSync(mcpTs))
    if (existsSync(serverTs)) await buildServerish(serverTs, join(outDir, 'server.js'))
    if (existsSync(mcpTs)) await buildServerish(mcpTs, join(outDir, 'mcp.js'))
    const files = listFiles(outDir)
    const cur: AppBuildCurrent = {
      key,
      status: 'ready',
      hostBuild,
      builtAt: Date.now(),
      bytes: dirBytes(outDir),
      files,
    }
    if (opts.pointer) {
      writeCurrent(slug, cur)
      // The pointer now names this build, so every older key is superseded:
      // the door would not serve them, and nothing loads them.
      pruneSupersededBuilds(slug)
    }
    return cur
  } catch (e) {
    const cur = failed(e instanceof Error ? e.message : String(e))
    if (opts.pointer) writeCurrent(slug, cur)
    return cur
  }
}
