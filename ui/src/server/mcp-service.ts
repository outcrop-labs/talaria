// The fleet's Talaria-toolkit MCP endpoint: talaria-mcp (mcp/dist) running in
// HTTP mode as a child of the app, one endpoint every agent's config points
// at (rendered by fleet-render). Self-supervising: ensureMcpService() is
// called opportunistically (renders, comms reads); it probes first so dev
// reloads and multiple entrypoints never double-spawn, and respawns on exit.
import { spawn } from 'node:child_process'
import { existsSync } from 'node:fs'
import { join, resolve } from 'node:path'

export const MCP_PORT = () => Number(process.env.TALARIA_MCP_PORT ?? 5280)
/** The URL agents reach the toolkit at (host-run app ⇒ host gateway). */
export const MCP_FLEET_URL = () => `http://host.docker.internal:${MCP_PORT()}/mcp`

const g = globalThis as unknown as { __talariaMcpService?: { starting: boolean; lastSpawn: number } }
const state = () => (g.__talariaMcpService ??= { starting: false, lastSpawn: 0 })

async function reachable(): Promise<boolean> {
  try {
    const r = await fetch(`http://127.0.0.1:${MCP_PORT()}/mcp`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      signal: AbortSignal.timeout(1_500),
    })
    return r.status !== 404 && r.status < 500 // 401/400 = alive, just unauthenticated
  } catch {
    return false
  }
}

/** Make sure the fleet MCP endpoint is up (spawn if not). Fire-and-forget. */
export function ensureMcpService(): void {
  const st = state()
  if (st.starting || Date.now() - st.lastSpawn < 10_000) return
  st.starting = true
  void (async () => {
    try {
      if (await reachable()) return
      const entry = resolve(process.cwd(), '../mcp/dist/index.js')
      if (!existsSync(entry)) {
        console.error(`[mcp-service] not built at ${entry} — run "npm run build" in mcp/`)
        return
      }
      st.lastSpawn = Date.now()
      const child = spawn(process.execPath, [entry], {
        env: {
          ...process.env,
          MCP_HTTP_PORT: String(MCP_PORT()),
          TALARIA_URL: `http://localhost:${process.env.PORT ?? 5273}`,
        },
        stdio: ['ignore', 'ignore', 'pipe'],
        detached: false,
      })
      child.stderr.on('data', (d: Buffer) => console.error(`[talaria-mcp] ${String(d).trim()}`))
      child.on('exit', (code) => console.error(`[mcp-service] exited (${code}) — will respawn on next ensure`))
    } finally {
      st.starting = false
    }
  })()
}

export const mcpServiceEntry = () => join(resolve(process.cwd(), '../mcp'), 'dist/index.js')
