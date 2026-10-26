// In-process MCP dispatcher for Talaria apps. An app's mcp.ts tools are
// registered in the MCP registry as a server row (app_slug set); the gateway
// resolves access with the SAME rules as any server, then dispatches here.
// The module is loaded from disk (dev) or the app's own build (prod) — never
// a host glob.
import { existsSync, mkdirSync, renameSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { storeFor } from './app-store'
import { rpcError, type Rpc } from './mcp-jsonrpc'
import type { AppMcp, AppMcpCaller } from '@/sdk/server'
import { catalogOf, checkAppMcpTools, runAppTools, type CallCut, type CheckedAppMcp } from './app-mcp-call'
import { isolateApp } from './app-isolate'
import { appHasMcp as hasMcpFile, loadAppMcp } from './app-load'
import { appDataDir } from './app-build/paths'
import { enabledAppSlugs } from './apps'
import { resyncAppMcpServers } from './internal-api'
import { appHarnessRunner } from './app-harness-runner'
import type { OrgToolCall } from './app-org-tools'
import type { ToolOutcome } from './harness/run'

export const appHasMcp = (slug: string): boolean => hasMcpFile(slug)

// The checks a tool passes, the catalog's shape and the call itself live in
// app-mcp-call.ts — a leaf `@talaria/sdk/testing` runs too, so a unit test
// calls a tool through exactly the code the gateway does. Re-exported for the
// probe (app-health.ts) and the existing callers.
export {
  TOOL_NAME_MAX,
  TOOL_DESCRIPTION_MAX,
  TOOL_SCHEMA_MAX_BYTES,
  argumentProblem,
  catalogOf,
  checkAppMcpTools,
  type CheckedAppMcp,
} from './app-mcp-call'

/** The registry row's `last_probe` for an app server, in the shape the api's
 *  `probe_json` writes: `ok` with the tool count, or `error` naming every
 *  tool left out — which Manage → MCP shows on the server's card. Clipped to
 *  the api's 300 characters. */
export function catalogProbe(checked: CheckedAppMcp, at: Date = new Date()): { state: 'ok' | 'error'; detail: string; at: string } {
  const n = checked.tools.length
  const count = `${n} tool${n === 1 ? '' : 's'}`
  if (!checked.problems.length) return { state: 'ok', detail: count, at: at.toISOString() }
  const full = `${count}; ${checked.problems.length} left out: ${checked.problems.join('; ')}`
  const detail = full.length > 300 ? `${full.slice(0, 299)}…` : full
  return { state: 'error', detail, at: at.toISOString() }
}

// The module's check, once per tools array: the same array comes back from
// the module cache on every dispatch until the module changes.
const checkedCache = new WeakMap<readonly unknown[], CheckedAppMcp>()

async function loadMcp(slug: string): Promise<CheckedAppMcp | null> {
  const loaded = await isolateApp(slug, 'mcp load', async () => {
    const mod = (await loadAppMcp(slug)) as { default?: AppMcp } | null
    const tools = mod?.default?.tools
    if (!Array.isArray(tools)) return null
    let checked = checkedCache.get(tools)
    if (!checked) {
      checked = checkAppMcpTools(tools)
      checkedCache.set(tools, checked)
      // A module this process has not seen — a first load, or an edit Vite
      // reloaded in dev, where no build probe runs — may carry a new catalog.
      // The sync writes and hands it on only when it changed.
      void syncAppMcpCatalog(slug, checked)
    }
    return checked
  })
  return loaded.ok ? loaded.value : null
}

export async function appMcpTools(slug: string): Promise<Array<{ name: string; description?: string }>> {
  const checked = await loadMcp(slug)
  return catalogOf(checked?.tools ?? []).map(({ name, description }) => ({ name, description }))
}

// ── The registry's cached catalog ───────────────────────────────────────────
// The registry row's `tools` column is what Manage → MCP's tool-subset picker
// and "Discover tools" read. Rust cannot load the module (rule 10), so the
// catalog travels the other way: the build probe writes the checked catalog
// to the app's data directory (`mcp-catalog.json`), and the api copies it
// into the row whenever it upserts one (talaria-apps `sync_app_mcp_servers`)
// — on enable, at boot, and when this host asks after writing a new catalog.
// An enabled app's row therefore has its tools the moment it exists; it used
// to start as `[]` and stay empty until some agent happened to call.
//
// NOT BESIDE THE BUILD. app-builds/ is served to anyone, unauthenticated and
// cached as immutable (server-entry.ts `tryServeAppBuilds`); the catalog is
// every tool's name, description and schema. app-data/ is never served.

/** registry twin: talaria-apps `MCP_CATALOG_FILE` / `mcp_catalog_path`. */
export const MCP_CATALOG_FILE = 'mcp-catalog.json'

export const appMcpCatalogPath = (slug: string): string => join(appDataDir(slug), MCP_CATALOG_FILE)

/** slug → the catalog body last written and handed to the api. */
const catalogWritten = new Map<string, string>()

/** Whether the enabled set names the app, read by the one rule every app
 *  gate uses (`enabledAppSlugs`, apps.ts): a stored value that is not a list
 *  of slugs throws. A lenient read matched a slug inside a malformed string
 *  value — `"crm-suite".includes("crm")`. */
const isEnabled = async (slug: string): Promise<boolean> => (await enabledAppSlugs()).includes(slug)

/** Write the checked catalog to the app's data directory and, for an enabled app,
 *  ask the api to copy it into the registry row. Never throws — a catalog that
 *  did not land is a warning (the api's boot sync reads the file anyway), not
 *  a failed load. */
export async function syncAppMcpCatalog(slug: string, c: CheckedAppMcp): Promise<void> {
  try {
    const tools = catalogOf(c.tools)
    const body = JSON.stringify({ tools, problems: c.problems })
    const file = appMcpCatalogPath(slug)
    // An uninstall removes the file: a remembered catalog whose file is gone
    // is written again.
    if (catalogWritten.get(slug) === body && existsSync(file)) return
    // 0700, as app-db.ts writeDbEnv makes it: the probe often reaches an
    // app's data directory before its database does, and mkdir never
    // tightens a directory that already exists.
    mkdirSync(appDataDir(slug), { recursive: true, mode: 0o700 })
    const at = new Date()
    writeFileSync(`${file}.tmp`, `${JSON.stringify({ tools, probe: catalogProbe(c, at), at: at.toISOString() }, null, 2)}\n`)
    renameSync(`${file}.tmp`, file)
    // Remembered only once the row has it (or there is no row): an api that
    // was down gets asked again on the next sync rather than never.
    if (!(await isEnabled(slug)) || (await resyncAppMcpServers(`${slug}: catalog`)) !== null) {
      catalogWritten.set(slug, body)
    }
  } catch (e) {
    console.warn(`[app-mcp] ${slug}: could not write the tool catalog: ${(e as Error).message}`)
  }
}

/** The app publishes no MCP surface (any more): drop its catalog, and let the
 *  api turn its row off if one was there. The row keeps its grants, so an
 *  mcp.ts that comes back brings the server back to the same agents. */
export async function clearAppMcpCatalog(slug: string): Promise<void> {
  catalogWritten.delete(slug)
  const file = appMcpCatalogPath(slug)
  if (!existsSync(file)) return
  rmSync(file, { force: true })
  if (await isEnabled(slug)) await resyncAppMcpServers(`${slug}: no MCP surface`)
}

/** Why the gateway must refuse this app server outright, or null.
 *
 *  AN APP THAT IS NOT ENABLED SERVES NOTHING, whatever its registry row says.
 *  Turning an app off (an admin, or a failed build) asks the api to turn the
 *  row off (`appOff`, keeping its grants) and roll the agents that carried it,
 *  but an agent's rendered config outlives that until its roll lands — and in
 *  dev the module loads from source whatever the build said, so a broken app's
 *  tools kept answering. The app server gateway (apps.$app.$.ts) has always
 *  applied this rule to people; this is the same rule for agents. A row the
 *  api turned off for any other reason (the app lost its mcp.ts) is refused
 *  with that reason. */
export async function appMcpOff(appSlug: string | null | undefined, appOff: string | null = null): Promise<string | null> {
  if (!appSlug) return null
  // An enabled set that cannot be read shuts the app's tools, as it shuts the
  // app's doors (authorizeApp answers 503): never read as enabled.
  const on = await isEnabled(appSlug).catch((e: Error) => {
    console.warn(`[app-mcp] ${appSlug}: could not read the enabled apps: ${e.message}`)
    return null
  })
  if (on === null) return `could not check that app "${appSlug}" is enabled — it serves nothing until that reads`
  if (!on) return `app "${appSlug}" is not enabled`
  return appOff ? `app "${appSlug}" is not serving: ${appOff}` : null
}

/** Who is calling, for whom, and until when — what the gateway knows about a
 *  call besides its arguments (app-mcp-call.ts `AppToolCallFacts`, less the
 *  store, which is the app's own). */
export interface AppMcpCallFacts {
  caller: AppMcpCaller
  actingUserId: string | null
  /** How a harness this call runs reaches an org MCP tool: as the calling
   *  agent, through the MCP gateway (app-org-tools.ts). Absent for a caller
   *  that lends no identity. */
  orgTools?: OrgToolCall | null
  /** The request's signal: aborts when the agent disconnects. */
  signal?: AbortSignal
  /** The server row's `timeoutSecs`; null is the 120 s default. */
  timeoutSecs?: number | null
}

export async function dispatchAppMcp(
  slug: string,
  rpc: Rpc,
  facts: AppMcpCallFacts,
  allowed: string[] | null,
): Promise<{ status: number; body: unknown | null; cut: CallCut }> {
  const mcp = await loadMcp(slug)
  if (!mcp) return { status: 404, body: rpcError(rpc.id, -32601, `app "${slug}" has no MCP surface`), cut: null }
  // Only the tools that passed their checks exist for an agent — listed,
  // callable, or not at all.
  const tools = mcp.tools.filter((t) => allowed === null || allowed.includes(t.name))
  // `ctx.harness`, bound to THIS caller: an agent pin only for the calling
  // agent, proven; the allowlist of the person it acts for. Each run follows
  // the tool's own signal (its deadline and the agent's disconnect), which
  // runAppTools adds per call.
  const harness = appHarnessRunner({
    app: slug,
    actor: { kind: 'mcp', caller: facts.caller, actingUserId: facts.actingUserId },
    orgTools: facts.orgTools ?? null,
  })
  const { orgTools: _org, ...call } = facts
  return runAppTools(slug, tools, rpc, { ...call, store: storeFor(slug), harness })
}

/** ONE OF THE APP'S OWN TOOLS, called from its own harness's loop
 *  (`loop.tools[].mcp`) — through the same dispatch an agent's call takes:
 *  the load checks, the inputSchema, the deadline, the result's shaping. What
 *  comes back is what the model sees. */
export async function callOwnAppTool(slug: string, name: string, args: Record<string, unknown>, facts: AppMcpCallFacts): Promise<ToolOutcome> {
  const rpc = { jsonrpc: '2.0', id: 1, method: 'tools/call', params: { name, arguments: args } } as Rpc
  const r = await dispatchAppMcp(slug, rpc, facts, null)
  const body = r.body as { error?: { message?: string }; result?: { content?: Array<{ type: string; text?: string }>; structuredContent?: unknown; isError?: boolean } } | null
  if (!body) return { text: `tool "${name}" was cut off before it answered`, isError: true }
  if (body.error) return { text: body.error.message ?? `tool "${name}" failed`, isError: true }
  const result = body.result ?? {}
  const text = (result.content ?? []).flatMap((b) => (b.type === 'text' && typeof b.text === 'string' ? [b.text] : [])).join('\n')
  return {
    text: text || (result.structuredContent !== undefined ? JSON.stringify(result.structuredContent) : ''),
    ...(result.structuredContent !== undefined ? { structured: result.structuredContent } : {}),
    isError: result.isError === true,
  }
}
