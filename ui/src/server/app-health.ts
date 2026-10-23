// Compile + load probe. Enablement is refused (and an already-enabled app
// is turned off) when this fails, with the error as the reason on current.json.
import { existsSync, readFileSync } from 'node:fs'
import { join } from 'node:path'
import { getSetting, logAudit, setSetting } from './audit'
import { isolateApp } from './app-isolate'
import { clientlessError, declaredSurfacesAt, slugOk } from './app-build/paths'
import { readCurrent, writeCurrent } from './app-build/status'
import {
  appClientArtifact,
  appHasClient,
  appHasMcp,
  appHasServer,
  liveTree,
  loadAppClient,
  loadAppMcp,
  loadAppServer,
  type AppTree,
} from './app-load'
import { checkAppHarnessesOf, recordAppHarnessReport } from './app-harnesses'
import { checkAppMcpTools, clearAppMcpCatalog, syncAppMcpCatalog } from './app-mcp'
import { resyncAppMcpServers } from './internal-api'


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

/** Built app.js is ESM. Node cannot import it (it wants /runtime/rt-*.js),
 *  so the contract checked here is the one the host relies on: a default
 *  export, the defineApp surfaces it mounts. */
export function clientArtifactOk(source: string): boolean {
  return source.length > 32 && /\bexport\s*(?:default\b|\{[^}]*\bas\s+default\b)/.test(source)
}


/** Mark the build failed and turn the app off. The enabled set is written
 *  here, but the app's MCP server row is the api's (talaria-apps): it is asked
 *  to reconcile, which turns the row OFF with the build's error and rolls the
 *  agents that carried it — otherwise a broken app's server stayed on every
 *  agent it had been granted to, until an admin happened to toggle some app.
 *  The row is never deleted here: a delete cascades every agent assignment,
 *  team grant and user grant on it, and a transient failure ("database did
 *  not start", a host upgrade that breaks every build at once) must not cost
 *  the org its app MCP access configuration. When the app serves again, the
 *  same row serves the same grants. Asked on every failure, not only when
 *  this call changed the set: a row an earlier failure left serving (the api
 *  was down) is turned off too. */
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
  await resyncAppMcpServers(`${slug}: ${next.length !== enabled.length ? 'turned off' : 'failed'}`)
}

/** Load server/mcp/client modules and demand the SDK contracts, and check the
 *  harnesses the app ships (never a reason to fail). Does not spawn Postgres.
 *
 *  The MCP module's tools are checked one by one (`checkAppMcpTools`): a tool
 *  that fails is left out and named — in the log and on the server's card in
 *  Manage → MCP — and never fails the probe. The checked catalog is written
 *  to the app's data directory and handed to the api, so the registry row
 *  carries the tools the moment the app is enabled. The boot sweep probes every installed
 *  app, which makes this the boot sync too.
 *
 *  `tree` probes code other than the live app's — an update's staged tree and
 *  its staged build — and `catalog: false` leaves the app's published
 *  catalog alone, so checking a version that is not live yet changes nothing
 *  agents see. */
export async function probeLoad(
  slug: string,
  opts: { tree?: AppTree; catalog?: boolean } = {},
): Promise<{ ok: true } | { ok: false; error: string }> {
  if (!slugOk(slug)) return { ok: false, error: 'bad slug' }
  const tree = opts.tree ?? liveTree(slug)
  const publish = opts.catalog !== false
  const has = (file: string) => existsSync(join(tree.dir, file))
  if (!appHasClient(slug, tree) && !has('app.ts')) {
    // No client: right for a server- or MCP-only app (talaria.json declares
    // no surfaces) — the rule build.ts applies to the build itself.
    const serverSide = appHasServer(slug, tree) || has('server.ts') || appHasMcp(slug, tree) || has('mcp.ts')
    const error = clientlessError(declaredSurfacesAt(tree.dir), serverSide)
    if (error) return { ok: false, error }
  } else {
    const client = await isolateApp(slug, 'load client', () => loadAppClient(slug, tree))
    if (!client.ok) return { ok: false, error: `app.ts failed to load: ${client.error}` }
    if (client.value) {
      if (!moduleHasSurfaces(client.value)) {
        return { ok: false, error: 'app.ts must default-export defineApp({ work | manage | settings })' }
      }
    } else {
      const artifact = appClientArtifact(slug, tree)
      if (!artifact) return { ok: false, error: 'app.js is missing from the current build' }
      if (!clientArtifactOk(readFileSync(artifact, 'utf8'))) {
        return { ok: false, error: 'app.js has no default export (defineApp)' }
      }
    }
  }
  if (appHasServer(slug, tree) || has('server.ts')) {
    const loaded = await isolateApp(slug, 'load server', () => loadAppServer(slug, tree))
    if (!loaded.ok) return { ok: false, error: `server failed to load: ${loaded.error}` }
    if (!moduleHasFetch(loaded.value)) {
      return { ok: false, error: 'server.ts must default-export defineAppServer({ fetch })' }
    }
  }
  if (appHasMcp(slug, tree) || has('mcp.ts')) {
    const loaded = await isolateApp(slug, 'load mcp', () => loadAppMcp(slug, tree))
    if (!loaded.ok) return { ok: false, error: `mcp failed to load: ${loaded.error}` }
    if (!moduleHasMcpTools(loaded.value)) {
      return { ok: false, error: 'mcp.ts must default-export defineAppMcp({ tools })' }
    }
    const checked = checkAppMcpTools((loaded.value as { default: { tools: unknown[] } }).default.tools)
    for (const problem of checked.problems) console.warn(`[app-mcp] ${slug}: left out ${problem}`)
    if (publish) await syncAppMcpCatalog(slug, checked)
  } else if (publish) {
    await clearAppMcpCatalog(slug)
  }
  // The harnesses under harnesses/ are checked here too, and like a bad MCP
  // tool a bad harness never fails the probe: it is skipped, and the reason
  // lands on the build status where Manage → Apps shows it.
  const harnesses = await isolateApp(slug, 'check harnesses', () => checkAppHarnessesOf(slug))
  recordAppHarnessReport(
    slug,
    harnesses.ok ? harnesses.value : { registered: [], problems: [`harnesses/: the check could not run: ${harnesses.error}`] },
  )
  return { ok: true }
}
