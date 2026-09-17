// One app = one standalone Vite build. Client chunk + CSS + optional
// server/mcp chunks, every shared specifier external and rewritten to the
// host's /runtime/rt-*.js entries. A bundled svelte is a build error
// (`effect_orphan` at runtime), not a surprise later.
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'
import { build as viteBuild, type Plugin } from 'vite'
import { svelte } from '@sveltejs/vite-plugin-svelte'
import tailwindcss from '@tailwindcss/vite'
import {
  bundledShared,
  clientSpecifiers,
  hostBuildStamp,
  isClientSpecifier,
  isServerSpecifier,
  shimFileName,
} from '@/app-runtime/specifiers'
import { sourceKey } from './key'
import { appBuildKeyDir, appSrc, uiRoot } from './paths'
import { readCurrent, writeCurrent, type AppBuildCurrent } from './status'

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
        external: (id) => isClientSpecifier(id),
        output: {
          format: 'es',
          entryFileNames: (c) => (c.name === 'app' ? 'app.js' : '_style.js'),
          assetFileNames: (a) => (typeof a.name === 'string' && a.name.endsWith('.css') ? 'app.css' : 'assets/[name][extname]'),
          paths: (id) => (isClientSpecifier(id) ? `/runtime/${shimFileName(clientSpecifiers().find((s) => id === s || id.startsWith(`${s}/`)) ?? id)}.js` : id),
        },
      },
    },
  })
  const leftover = join(outDir, '_style.js')
  if (existsSync(leftover)) rmSync(leftover)
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
          entryFileNames: outFile.endsWith('mcp.js') ? 'mcp.js' : 'server.js',
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

/** Compile one app into `<builds>/<slug>/<key>/`. Idempotent when the key matches. */
export async function buildApp(slug: string): Promise<AppBuildCurrent> {
  const src = appSrc(slug, 'talaria.json')
  if (!existsSync(src)) throw new Error(`no app "${slug}"`)
  const hostBuild = hostId()
  const key = sourceKey(join(src, '..'), hostBuild)
  const existing = readCurrent(slug)
  if (existing?.status === 'ready' && existing.key === key) return existing
  writeCurrent(slug, { key, status: 'building', hostBuild, builtAt: Date.now(), bytes: 0, files: [] })
  const outDir = appBuildKeyDir(slug, key)
  mkdirSync(outDir, { recursive: true })
  try {
    await buildClient(join(src, '..'), outDir)
    const serverTs = appSrc(slug, 'server.ts')
    if (existsSync(serverTs)) await buildServerish(serverTs, join(outDir, 'server.js'))
    const mcpTs = appSrc(slug, 'mcp.ts')
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
    writeCurrent(slug, cur)
    return cur
  } catch (e) {
    const cur: AppBuildCurrent = {
      key,
      status: 'failed',
      hostBuild,
      builtAt: Date.now(),
      bytes: 0,
      files: [],
      error: e instanceof Error ? e.message : String(e),
    }
    writeCurrent(slug, cur)
    return cur
  }
}
