// In-process MCP dispatcher for Talaria apps. An app's mcp.ts tools are
// registered in the MCP registry as a server row (app_slug set); the gateway
// resolves access with the SAME rules as any server, then dispatches here.
// The module is loaded from disk (dev) or the app's own build (prod) — never
// a host glob.
import { existsSync, mkdirSync, renameSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { z } from 'zod'
import { storeFor } from './app-store'
import { dispatchJsonRpc, rpcError, type Rpc } from './mcp-jsonrpc'
import type { AppMcp, AppMcpTool } from '@/sdk/server'
import { isolateApp } from './app-isolate'
import { appHasMcp as hasMcpFile, loadAppMcp } from './app-load'
import { appDataDir } from './app-build/paths'
import { getSetting } from './audit'
import { resyncAppMcpServers } from './internal-api'

export const appHasMcp = (slug: string): boolean => hasMcpFile(slug)

// ── The checks a tool passes before any agent sees it ──────────────────────
// Every tool an app declares is checked when its module loads — by the build
// probe, and by every dispatch. A tool that fails is LEFT OUT and named: not
// listed, not callable, not in the registry's catalog. It never takes the app
// (or the app's other tools) down with it; a typo in one tool is that tool's
// problem. The bounds are the registry's own (`bound_catalog`,
// api/crates/talaria-mcp/src/registry.rs), which used to clip a third-party
// catalog silently and let an app's through unchecked.

/** registry.rs `TOOL_NAME_MAX`. */
export const TOOL_NAME_MAX = 200
/** registry.rs `TOOL_DESCRIPTION_MAX`: the catalog keeps this much. */
export const TOOL_DESCRIPTION_MAX = 300
/** registry.rs `TOOL_SCHEMA_MAX_BYTES`. */
export const TOOL_SCHEMA_MAX_BYTES = 32 * 1024
/** What a model API accepts as a function name. An agent's harness turns each
 *  MCP tool into one, and providers refuse the WHOLE request over a name with
 *  a space or a dot in it — so one bad name would break every turn of every
 *  agent carrying the app, not just calls to that tool. */
const TOOL_NAME_RE = /^[A-Za-z0-9_-]+$/

const EMPTY_SCHEMA = { type: 'object', properties: {} } as const

export interface CheckedAppMcp {
  /** The tools that passed — the only ones listed, called or catalogued. */
  tools: AppMcpTool[]
  /** One sentence per tool left out: which one, and why. */
  problems: string[]
}

function toolProblem(t: unknown, seen: ReadonlySet<string>): string | null {
  if (!t || typeof t !== 'object' || Array.isArray(t)) return 'is not a tool definition'
  const { name, description, handler, inputSchema } = t as Partial<Record<keyof AppMcpTool, unknown>>
  if (typeof name !== 'string' || !name) return 'has no name'
  if (name.length > TOOL_NAME_MAX) return `has a ${name.length}-character name (at most ${TOOL_NAME_MAX})`
  if (!TOOL_NAME_RE.test(name)) return 'has a name with characters other than letters, digits, _ and -'
  if (seen.has(name)) return 'is defined twice; only the first definition is kept'
  if (typeof description !== 'string' || !description.trim()) return 'has no description'
  if (typeof handler !== 'function') return 'has no handler function'
  if (inputSchema !== undefined) {
    if (!inputSchema || typeof inputSchema !== 'object' || Array.isArray(inputSchema)) {
      return 'has an inputSchema that is not a JSON Schema object'
    }
    const type = (inputSchema as Record<string, unknown>).type
    if (type !== undefined && type !== 'object') {
      return `has an inputSchema of type ${JSON.stringify(type)}; tool arguments are an object`
    }
    let bytes: number
    try {
      bytes = Buffer.byteLength(JSON.stringify(inputSchema))
    } catch {
      return 'has an inputSchema that is not JSON'
    }
    if (bytes > TOOL_SCHEMA_MAX_BYTES) {
      return `has a ${(bytes / 1024).toFixed(1)} KiB inputSchema (at most ${TOOL_SCHEMA_MAX_BYTES / 1024} KiB)`
    }
  }
  return null
}

const toolLabel = (t: unknown, i: number): string => {
  const name = t && typeof t === 'object' ? (t as { name?: unknown }).name : undefined
  return typeof name === 'string' && name ? JSON.stringify(name.length > 60 ? `${name.slice(0, 60)}…` : name) : `tool #${i + 1}`
}

/** Check every declared tool; keep the ones that pass, name the rest. */
export function checkAppMcpTools(tools: readonly unknown[]): CheckedAppMcp {
  const ok: AppMcpTool[] = []
  const problems: string[] = []
  const seen = new Set<string>()
  tools.forEach((t, i) => {
    const problem = toolProblem(t, seen)
    if (problem) {
      problems.push(`${toolLabel(t, i)} ${problem}`)
      return
    }
    const tool = t as AppMcpTool
    seen.add(tool.name)
    ok.push(tool)
  })
  return { tools: ok, problems }
}

/** The catalog the registry row holds for an app server: what tools/list
 *  serves, with descriptions clipped to the registry's bound. */
export const catalogOf = (tools: readonly AppMcpTool[]): Array<{ name: string; description: string; inputSchema: Record<string, unknown> }> =>
  tools.map((t) => ({
    name: t.name,
    description: t.description.slice(0, TOOL_DESCRIPTION_MAX),
    inputSchema: t.inputSchema ?? { ...EMPTY_SCHEMA },
  }))

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

const isEnabled = async (slug: string): Promise<boolean> =>
  (await getSetting<string[]>('apps_enabled', [])).includes(slug)

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
    mkdirSync(appDataDir(slug), { recursive: true })
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

// ── Argument validation against the advertised inputSchema ─────────────────
// `inputSchema` was advisory: advertised in tools/list, never checked, so the
// handler was the first code to see a wrong shape. zod's `fromJSONSchema`
// turns the JSON Schema into a parser (extra properties pass unless the
// schema forbids them, as JSON Schema says); a schema it cannot convert
// ($ref, a draft it does not know) falls back to no check rather than
// refusing every call to that tool. Compiled once per schema object.
const compiled = new WeakMap<object, z.ZodType | null>()

export function argumentProblem(schema: Record<string, unknown> | undefined, args: Record<string, unknown>): string | null {
  if (!schema) return null
  let parser = compiled.get(schema)
  if (parser === undefined) {
    try {
      parser = z.fromJSONSchema(schema as Parameters<typeof z.fromJSONSchema>[0])
    } catch {
      parser = null
    }
    compiled.set(schema, parser)
  }
  if (!parser) return null
  const r = parser.safeParse(args)
  return r.success ? null : z.prettifyError(r.error)
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
  if (!(await isEnabled(appSlug))) return `app "${appSlug}" is not enabled`
  return appOff ? `app "${appSlug}" is not serving: ${appOff}` : null
}

export async function dispatchAppMcp(
  slug: string,
  rpc: Rpc,
  agent: string,
  allowed: string[] | null,
): Promise<{ status: number; body: unknown | null }> {
  const mcp = await loadMcp(slug)
  if (!mcp) return { status: 404, body: rpcError(rpc.id, -32601, `app "${slug}" has no MCP surface`) }
  // Only the tools that passed their checks exist for an agent — listed,
  // callable, or not at all.
  const tools = mcp.tools.filter((t) => allowed === null || allowed.includes(t.name))

  return dispatchJsonRpc(rpc, tools, {
    serverName: `talaria-app-${slug}`,
    listEntry: (t: AppMcpTool) => ({
      name: t.name,
      description: t.description,
      inputSchema: t.inputSchema ?? EMPTY_SCHEMA,
    }),
    call: async (tool, args) => {
      const bad = argumentProblem(tool.inputSchema, args)
      if (bad) return { text: `invalid arguments for ${tool.name}:\n${bad}`, isError: true }
      const out = await tool.handler(args, { app: slug, agent, store: storeFor(slug) })
      return { text: typeof out === 'string' ? out : JSON.stringify(out ?? null, null, 2) }
    },
  })
}
