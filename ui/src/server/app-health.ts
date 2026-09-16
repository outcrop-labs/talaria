// Compile + load probe. Enablement is refused (and an already-enabled app
// is turned off) when this fails, with the error as the reason on current.json.
import { existsSync, readFileSync } from 'node:fs'
import { getSetting, logAudit, setSetting } from './audit'
import { isolateApp } from './app-isolate'
import { appSrc, slugOk } from './app-build/paths'
import { readCurrent, writeCurrent } from './app-build/status'
import {
  appClientArtifact,
  appHasClient,
  appHasMcp,
  appHasServer,
  loadAppClient,
  loadAppMcp,
  loadAppServer,
} from './app-load'


export function moduleHasFetch(mod: unknown): boolean {
  if (!mod || typeof mod !== 'object' || !('default' in mod)) return false
  const exported = mod.default
  if (!exported || typeof exported !== 'object' || !('fetch' in exported)) return false
  return typeof exported.fetch === 'function'
}

export function moduleHasMcpTools(mod: unknown): boolean {
  if (!mod || typeof mod !== 'object' || !('default' in mod)) return false
  const exported = mod.default
  if (!exported || typeof exported !== 'object' || !('tools' in exported)) return false
  return Array.isArray(exported.tools)
}

export function moduleHasSurfaces(mod: unknown): boolean {
  if (!mod || typeof mod !== 'object' || !('default' in mod)) return false
  const exported = mod.default
  if (!exported || typeof exported !== 'object') return false
  const surfaces = exported as Record<string, unknown>
  return ['work', 'manage', 'settings'].some((k) => surfaces[k] != null)
}

/** Built app.js is ESM. Node cannot import it (it wants /runtime/rt-*.js);
 *  presence of an export is the compile contract we can check here. */
export function clientArtifactOk(source: string): boolean {
  return source.length > 32 && /\bexport\b/.test(source)
}


export async function failHealth(slug: string, error: string): Promise<void> {
  const cur = readCurrent(slug)
  writeCurrent(slug, {
    key: cur?.key ?? '',
    status: 'failed',
    hostBuild: cur?.hostBuild ?? '',
    builtAt: Date.now(),
    bytes: cur?.bytes ?? 0,
    files: cur?.files ?? [],
    error,
  })
  const enabled = await getSetting<string[]>('apps_enabled', [])
  const next = enabled.filter((s) => s !== slug).sort()
  if (next.length !== enabled.length) {
    await setSetting('apps_enabled', next)
    await logAudit({
      actor: 'system:app-health',
      action: 'app.disable',
      targetType: 'app',
      targetId: slug,
      targetLabel: error,
    })
  }
}

/** Load server/mcp/client modules and demand the SDK contracts. Does not spawn Postgres. */
export async function probeLoad(slug: string): Promise<{ ok: true } | { ok: false; error: string }> {
  if (!slugOk(slug)) return { ok: false, error: 'bad slug' }
  if (!appHasClient(slug) && !existsSync(appSrc(slug, 'app.ts'))) {
    return { ok: false, error: 'app.ts is required' }
  }
  const client = await isolateApp(slug, 'load client', () => loadAppClient(slug))
  if (!client.ok) return { ok: false, error: `app.ts failed to load: ${client.error}` }
  if (client.value) {
    if (!moduleHasSurfaces(client.value)) {
      return { ok: false, error: 'app.ts must default-export defineApp({ work | manage | settings })' }
    }
  } else {
    const artifact = appClientArtifact(slug)
    if (!artifact) return { ok: false, error: 'app.js is missing from the current build' }
    if (!clientArtifactOk(readFileSync(artifact, 'utf8'))) {
      return { ok: false, error: 'app.js is not an ESM client bundle' }
    }
  }
  if (appHasServer(slug) || existsSync(appSrc(slug, 'server.ts'))) {
    const loaded = await isolateApp(slug, 'load server', () => loadAppServer(slug))
    if (!loaded.ok) return { ok: false, error: `server failed to load: ${loaded.error}` }
    if (!moduleHasFetch(loaded.value)) {
      return { ok: false, error: 'server.ts must default-export defineAppServer({ fetch })' }
    }
  }
  if (appHasMcp(slug) || existsSync(appSrc(slug, 'mcp.ts'))) {
    const loaded = await isolateApp(slug, 'load mcp', () => loadAppMcp(slug))
    if (!loaded.ok) return { ok: false, error: `mcp failed to load: ${loaded.error}` }
    if (!moduleHasMcpTools(loaded.value)) {
      return { ok: false, error: 'mcp.ts must default-export defineAppMcp({ tools })' }
    }
  }
  return { ok: true }
}
