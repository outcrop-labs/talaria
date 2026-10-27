// `talaria-app build`: the host's build recipe and load probe, run in the
// sandbox, reported per surface and per harness the way Manage → Apps would.
//
// Not a re-implementation. `buildAppAt` is app-build/build.ts, `probeLoad` is
// app-health.ts, the harness report is app-harnesses.ts — the same functions
// the instance runs when an admin installs or rebuilds the app, bundled into
// the kit. What differs is only where they read and write (workspace.ts) and
// that nothing here has a database: the probe runs with `catalog: false`, so
// it publishes no MCP catalog and refreshes no other app's report.
import { existsSync } from 'node:fs'
import { join, relative } from 'node:path'
import { buildAppAt } from '@/server/app-build/build'
import { appBuildKeyDir, declaredSurfacesAt } from '@/server/app-build/paths'
import { readCurrent, type AppBuildCurrent } from '@/server/app-build/status'
import type { SurfaceKey } from '@/server/app-build/manifest'
import { probeLoad } from '@/server/app-health'
import { loadAppMcp, type AppTree } from '@/server/app-load'
import { checkAppMcpTools } from '@/server/app-mcp-call'
import type { KitWorkspace } from './workspace'

export interface BuildReport {
  app: { name: string; slug: string; dir: string }
  kit: string
  build: {
    status: 'ready' | 'failed'
    key: string
    /** Where the build landed — the work area, outside the app. */
    out: string | null
    files: string[]
    bytes: number
    error: string | null
  }
  /** The surfaces talaria.json declares, and whether app.js was built. */
  client: { declared: SurfaceKey[]; built: boolean }
  server: boolean
  /** The tools the probe keeps, and each it leaves out with why. */
  mcp: { tools: string[]; leftOut: string[] } | null
  /** What the probe made of harnesses/: the ids it registers, and one
   *  sentence per file it skipped — the report Manage → Apps shows. */
  harnesses: { registered: string[]; skipped: string[] } | null
  /** The load probe. Null when the build failed, so there was nothing to
   *  load. A failure is what disables the app on the host. */
  probe: { ok: true } | { ok: false; error: string } | null
  /** The build and the probe passed. A skipped harness or a tool left out
   *  does not fail an app on the host, and does not fail this. */
  ok: boolean
}

/** What the host's code printed while it ran, kept for `--verbose` rather
 *  than interleaved with the report. */
export async function captureConsole<T>(fn: () => Promise<T>): Promise<{ value: T; lines: string[] }> {
  const lines: string[] = []
  const saved = { log: console.log, info: console.info, warn: console.warn, error: console.error }
  const keep = (...args: unknown[]) => {
    lines.push(args.map((a) => (a instanceof Error ? (a.stack ?? a.message) : typeof a === 'string' ? a : JSON.stringify(a))).join(' '))
  }
  console.log = keep
  console.info = keep
  console.warn = keep
  console.error = keep
  try {
    return { value: await fn(), lines }
  } finally {
    Object.assign(console, saved)
  }
}

/** Build and probe the app in `ws`. */
export async function kitBuild(ws: KitWorkspace, appName: string, kitVersion: string): Promise<BuildReport> {
  const cur = await buildAppAt(ws.slug, ws.app, { pointer: true })
  const declared = declaredSurfacesAt(ws.app)
  const report: BuildReport = {
    app: { name: appName, slug: ws.slug, dir: ws.app },
    kit: kitVersion,
    build: {
      status: cur.status === 'ready' ? 'ready' : 'failed',
      key: cur.key,
      out: cur.status === 'ready' ? appBuildKeyDir(ws.slug, cur.key) : null,
      files: cur.files,
      bytes: cur.bytes,
      error: cur.status === 'ready' ? null : (cur.error ?? 'the build failed'),
    },
    client: { declared, built: cur.files.includes('app.js') },
    server: cur.files.includes('server.js'),
    mcp: null,
    harnesses: null,
    probe: null,
    ok: false,
  }
  if (cur.status !== 'ready') return report

  const tree = (): AppTree => ({ slug: ws.slug, dir: ws.app, build: readCurrent(ws.slug) })
  report.probe = await probeLoad(ws.slug, { tree: tree(), catalog: false })

  if (cur.files.includes('mcp.js')) {
    // Imported once already by the probe; this reads the same module back.
    try {
      const mod = (await loadAppMcp(ws.slug, tree())) as { default?: { tools?: unknown } } | null
      const tools = Array.isArray(mod?.default?.tools) ? (mod.default.tools as unknown[]) : []
      const checked = checkAppMcpTools(tools)
      report.mcp = { tools: checked.tools.map((t) => t.name), leftOut: checked.problems }
    } catch (e) {
      report.mcp = { tools: [], leftOut: [`mcp.js could not be loaded: ${e instanceof Error ? e.message : String(e)}`] }
    }
  }
  const after: AppBuildCurrent | null = readCurrent(ws.slug)
  if (after?.harnesses) report.harnesses = { registered: after.harnesses.registered, skipped: after.harnesses.problems }
  else if (existsSync(join(ws.app, 'harnesses')) && cur.files.includes('harnesses.js')) report.harnesses = { registered: [], skipped: [] }
  report.ok = report.probe.ok
  return report
}

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`

const kb = (bytes: number) => (bytes < 1024 ? `${bytes} B` : `${(bytes / 1024).toFixed(1)} KB`)

/** The report as a person (or an agent) reads it. */
export function formatBuildReport(r: BuildReport, cwd: string = process.cwd()): string {
  const out: string[] = []
  const row = (label: string, text: string) => out.push(`  ${label.padEnd(10)} ${text}`)
  const sub = (text: string) => out.push(`  ${' '.repeat(10)}   ${text}`)
  out.push(`${r.app.name} (${r.app.slug}) · kit ${r.kit}`)
  if (r.build.status === 'failed') {
    row('build', `FAILED — ${r.build.error}`)
    out.push('', 'The host refuses this build the same way: an install or update would fail, and an')
    out.push('enabled app would be turned off.')
    return out.join('\n')
  }
  row('build', `ready · key ${r.build.key} · ${plural(r.build.files.length, 'file')} · ${kb(r.build.bytes)}`)
  if (r.build.out) sub(relative(cwd, r.build.out).startsWith('..') ? r.build.out : relative(cwd, r.build.out))
  row('client', r.client.declared.length ? `app.js · surfaces: ${r.client.declared.join(', ')}` : r.client.built ? 'app.js · no surfaces declared' : 'none (no surfaces declared)')
  row('server', r.server ? 'server.js' : 'none')
  if (r.mcp) {
    row('mcp', r.mcp.tools.length ? `${plural(r.mcp.tools.length, 'tool')}: ${r.mcp.tools.join(', ')}` : 'no tools pass the load checks')
    for (const p of r.mcp.leftOut) sub(`left out: ${p}`)
  } else row('mcp', 'none')
  if (r.harnesses) {
    const { registered, skipped } = r.harnesses
    row('harnesses', `${registered.length ? `registered: ${registered.join(', ')}` : 'none registered'}${skipped.length ? ` · ${skipped.length} skipped` : ''}`)
    for (const p of skipped) sub(`skipped: ${p}`)
  } else row('harnesses', 'none')
  if (r.probe && !r.probe.ok) {
    row('probe', `FAILED — ${r.probe.error}`)
    out.push('', 'The host runs this probe on every install, update and enable; a failure turns the app off.')
  } else row('probe', 'ok')
  return out.join('\n')
}
