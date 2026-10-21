// In-process MCP dispatcher for Talaria apps. An app's mcp.ts tools are
// registered in the MCP registry as a server row (app_slug set); the gateway
// resolves access with the SAME rules as any server, then dispatches here.
// The module is loaded from disk (dev) or the app's own build (prod) — never
// a host glob.
import { z } from 'zod'
import { storeFor } from './app-store'
import { dispatchJsonRpc, rpcError, type Rpc } from './mcp-jsonrpc'
import type { AppMcp, AppMcpTool } from '@/sdk/server'
import { isolateApp } from './app-isolate'
import { appHasMcp as hasMcpFile, loadAppMcp } from './app-load'

export const appHasMcp = (slug: string): boolean => hasMcpFile(slug)

async function loadMcp(slug: string): Promise<AppMcp | null> {
  const loaded = await isolateApp(slug, 'mcp load', async () => {
    const mod = (await loadAppMcp(slug)) as { default?: AppMcp } | null
    return mod?.default?.tools ? mod.default : null
  })
  if (!loaded.ok || !loaded.value) return null
  // Every successful load — the build probe's, the first dispatch's — is a
  // chance to write the catalog the registry row was seeded without.
  void syncAppMcpCatalog(slug, loaded.value)
  return loaded.value
}

export async function appMcpTools(slug: string): Promise<Array<{ name: string; description?: string }>> {
  const mcp = await loadMcp(slug)
  return catalogOf(mcp)
}

const catalogOf = (mcp: AppMcp | null): Array<{ name: string; description?: string }> =>
  (mcp?.tools ?? []).map((t) => ({ name: t.name, description: t.description.slice(0, 300) }))

// ── The registry's cached catalog ───────────────────────────────────────────
// The Rust side registers an app server with `tools = '[]'` and never
// refreshes it (no probe: the module is the authority, and this process is
// the only one that can load it). The gateway serves tools/list from the
// module on every call, so agents never noticed — but Manage → MCP's "narrow
// the tool subset" picker reads the cached column, and an empty catalog is a
// picker with nothing to pick. This writes the column from the module once
// per distinct catalog per process, and again whenever a load finds the row
// was not there yet (install → build runs before enable → register).

/** slug → the catalog JSON last written for it. */
const catalogWritten = new Map<string, string>()

/** Has this process already written this app's current catalog into the
 *  registry row? False until a load has run and found the row. */
export const appMcpCatalogSynced = (slug: string): boolean => catalogWritten.has(slug)

/** Write the module's tool list into `mcp_servers.tools` for `app-<slug>`.
 *  Pass the loaded module to avoid a second load; without one it loads (and
 *  the load syncs). Never throws — a catalog that did not land is a warning,
 *  not a failed call. */
export async function syncAppMcpCatalog(slug: string, mcp?: AppMcp | null): Promise<void> {
  try {
    if (mcp === undefined) {
      await loadMcp(slug) // the load writes
      return
    }
    if (!mcp) return
    const body = JSON.stringify(catalogOf(mcp))
    if (catalogWritten.get(slug) === body) return
    const { db } = await import('./db/pg')
    const sql = await db()
    const r = await sql.unsafe(
      `update mcp_servers set tools = $1::jsonb, tools_refreshed_at = now(), updated_at = now()
       where app_slug = $2 and name = $3`,
      [body, slug, `app-${slug}`],
    )
    // No row yet (the app is built but not enabled): try again on the next
    // load rather than remembering a write that never happened.
    if (r.count > 0) catalogWritten.set(slug, body)
  } catch (e) {
    console.warn(`[app-mcp] ${slug}: could not write the tool catalog to the registry: ${(e as Error).message}`)
  }
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

export async function dispatchAppMcp(
  slug: string,
  rpc: Rpc,
  agent: string,
  allowed: string[] | null,
): Promise<{ status: number; body: unknown | null }> {
  const mcp = await loadMcp(slug)
  if (!mcp) return { status: 404, body: rpcError(rpc.id, -32601, `app "${slug}" has no MCP surface`) }
  const tools = mcp.tools.filter((t) => allowed === null || allowed.includes(t.name))

  return dispatchJsonRpc(rpc, tools, {
    serverName: `talaria-app-${slug}`,
    listEntry: (t: AppMcpTool) => ({
      name: t.name,
      description: t.description,
      inputSchema: t.inputSchema ?? { type: 'object', properties: {} },
    }),
    call: async (tool, args) => {
      const bad = argumentProblem(tool.inputSchema, args)
      if (bad) return { text: `invalid arguments for ${tool.name}:\n${bad}`, isError: true }
      const out = await tool.handler(args, { app: slug, agent, store: storeFor(slug) })
      return { text: typeof out === 'string' ? out : JSON.stringify(out ?? null, null, 2) }
    },
  })
}
