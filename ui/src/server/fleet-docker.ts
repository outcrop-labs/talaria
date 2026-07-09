// Docker control for the managed fleet. Talaria drives `docker compose` on the
// rendered fleet/docker-compose.yml (interpolation env = the fleet's own .env,
// so per-agent keys/secrets stay in one Talaria-owned, gitignored place).
import { execFile } from 'node:child_process'
import { promisify } from 'node:util'
import { join } from 'node:path'
import { db } from './db/pg'
import { FLEET_DIR, FLEET_ENV, fleetNetworkName } from './fleet-render'

const run = promisify(execFile)

// Rolling slots: each agent runs as `agent-<dept>` (slot a) or `agent-<dept>-b`
// (slot b). Callers pass a department as always — the active slot resolves here
// — and only the roll orchestration addresses a slot explicitly.
export type Slot = 'a' | 'b'
export const slotService = (department: string, slot: Slot) => `agent-${department}${slot === 'b' ? '-b' : ''}`
export const slotContainer = (department: string, slot: Slot) => `talaria-fleet-${slotService(department, slot)}-1`

async function activeSlot(department: string): Promise<Slot> {
  const sql = await db()
  const rows = (await sql`
    select active_slot as s from agent_defs where department = ${department} and managed limit 1
  `) as unknown as Array<{ s: string }>
  return rows[0]?.s === 'b' ? 'b' : 'a'
}

/** Remove a container by exact name (used to retire the old slot after a roll —
 *  it's no longer in the compose file by then, so compose can't address it). */
export async function removeContainerByName(name: string): Promise<void> {
  await run('docker', ['rm', '-f', name], { timeout: 60_000 })
}

/** The fleet joins an EXTERNAL network (shared with app/bridge/MCP containers),
 *  which compose declares but never creates. Create it here when missing so a
 *  fresh install works without any setup script. Idempotent, race-safe. */
async function ensureFleetNetwork(): Promise<void> {
  const name = await fleetNetworkName()
  try {
    await run('docker', ['network', 'inspect', name], { timeout: 10_000 })
  } catch {
    await run('docker', ['network', 'create', name], { timeout: 10_000 }).catch(async () => {
      // lost a create race, or a real failure — only the latter should throw
      await run('docker', ['network', 'inspect', name], { timeout: 10_000 })
    })
  }
}

const composeArgs = (args: string[]) => [
  'compose',
  '-p',
  'talaria-fleet',
  '-f',
  join(FLEET_DIR(), 'docker-compose.yml'),
  '--env-file',
  FLEET_ENV(),
  ...args,
]

export async function fleetUp(department: string, slot?: Slot): Promise<string> {
  await ensureFleetNetwork()
  const svc = slotService(department, slot ?? (await activeSlot(department)))
  const { stderr } = await run('docker', composeArgs(['up', '-d', svc]), { timeout: 120_000 })
  return stderr.trim()
}

export async function fleetStop(department: string, slot?: Slot): Promise<string> {
  const svc = slotService(department, slot ?? (await activeSlot(department)))
  const { stderr } = await run('docker', composeArgs(['stop', svc]), { timeout: 60_000 })
  return stderr.trim()
}

/** Restart a managed agent so a re-rendered config.yaml takes effect. Prefer
 *  rollAgent (fleet-reconcile) for user-facing changes — a restart has a
 *  downtime window; a roll doesn't. */
export async function fleetRestart(department: string, slot?: Slot): Promise<string> {
  const svc = slotService(department, slot ?? (await activeSlot(department)))
  const { stderr } = await run('docker', composeArgs(['restart', svc]), { timeout: 120_000 })
  return stderr.trim()
}

export async function fleetRemove(department: string, slot?: Slot): Promise<string> {
  const svc = slotService(department, slot ?? (await activeSlot(department)))
  const { stderr } = await run('docker', composeArgs(['rm', '-sf', svc]), { timeout: 60_000 })
  return stderr.trim()
}

export interface ContainerState {
  name: string
  state: string // running | exited | …
  status: string // human string incl. health, e.g. "Up 2 hours (healthy)"
}

export interface AgentContainers {
  department: string
  managed: ContainerState | null
}

/** Container reality per department: the talaria-managed service, by name —
 *  either slot counts, preferring the one that's running (mid-roll both exist). */
export async function containerStatus(departments: string[]): Promise<AgentContainers[]> {
  const { stdout } = await run('docker', ['ps', '-a', '--format', '{{json .}}'], { timeout: 20_000 })
  const all = stdout
    .split('\n')
    .filter(Boolean)
    .map((l) => JSON.parse(l) as { Names: string; State: string; Status: string })
  const byName = new Map(all.map((c) => [c.Names, { name: c.Names, state: c.State, status: c.Status }]))
  return departments.map((department) => {
    const a = byName.get(slotContainer(department, 'a'))
    const b = byName.get(slotContainer(department, 'b'))
    return { department, managed: (a?.state === 'running' ? a : b?.state === 'running' ? b : (a ?? b)) ?? null }
  })
}

/** Wait for the managed container to report healthy (or give up). */
export async function waitHealthy(department: string, slot?: Slot, timeoutMs = 120_000): Promise<boolean> {
  const name = slotContainer(department, slot ?? (await activeSlot(department)))
  const deadline = Date.now() + timeoutMs
  while (Date.now() < deadline) {
    try {
      const { stdout } = await run('docker', ['inspect', '-f', '{{.State.Health.Status}}', name], { timeout: 10_000 })
      const s = stdout.trim()
      if (s === 'healthy') return true
      if (s === 'unhealthy') return false
    } catch {
      /* container not created yet */
    }
    await new Promise((r) => setTimeout(r, 3000))
  }
  return false
}
