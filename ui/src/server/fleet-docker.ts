// Docker control for the managed fleet. Talaria drives `docker compose` on the
// rendered fleet/docker-compose.yml (interpolation env = the fleet's own .env,
// so per-agent keys/secrets stay in one Talaria-owned, gitignored place).
import { execFile } from 'node:child_process'
import { promisify } from 'node:util'
import { join } from 'node:path'
import { db } from './db/pg'
import { FLEET_DIR, FLEET_ENV, fleetNetworkName, fleetProject } from './fleet-render'

const run = promisify(execFile)

// Rolling slots: each agent runs as `agent-<dept>` (slot a) or `agent-<dept>-b`
// (slot b). Callers pass a department as always — the active slot resolves here
// — and only the roll orchestration addresses a slot explicitly.
export type Slot = 'a' | 'b'
export const slotService = (department: string, slot: Slot) => `agent-${department}${slot === 'b' ? '-b' : ''}`
export const slotContainer = (department: string, slot: Slot) => `${fleetProject()}-${slotService(department, slot)}-1`

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

/** The managed container's CURRENT name — anything that docker-execs into an
 *  agent must resolve through this, never hardcode the slot-a name (a rolled
 *  agent lives in "-b-1" until its next roll). */
export async function managedContainer(department: string): Promise<string> {
  return slotContainer(department, await activeSlot(department))
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
  fleetProject(),
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
  // Bringing an agent up is the moment to ask whether it can reach us — from
  // the fleet network, not from here. Detached: a preflight must never be able
  // to fail a start, and its whole value is that it writes a verdict somebody
  // reads later (alerts.ts) rather than that this caller waits for it.
  void import('./fleet-preflight')
    .then((m) => m.runFleetPreflight())
    .catch(() => {})
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
  state: string // running | exited |
  status: string // human string incl. health, e.g. "Up 2 hours (healthy)"
  /** Parsed healthcheck phase — 'starting' is the warm-up window
   *  (start_period 60s on the compose healthcheck). null = no health info. */
  health: 'starting' | 'healthy' | 'unhealthy' | null
}

/** docker ps folds health into the Status string — "(health: starting)" /
 *  "(healthy)" / "(unhealthy)". Parse it out so callers get a real state. */
function parseHealth(status: string): ContainerState['health'] {
  if (/health: starting/i.test(status)) return 'starting'
  if (/\(healthy\)/i.test(status)) return 'healthy'
  if (/unhealthy/i.test(status)) return 'unhealthy'
  return null
}

export interface AgentContainers {
  department: string
  managed: ContainerState | null
}

/** Container reality per department: the talaria-managed service, by name —
 *  either slot counts, preferring the one that's running (mid-roll both exist).
 *  Docker CLI costs ~0.5s per shell-out and Home + alerts + the roster all
 *  ask within the same breath — a 5s cache dedupes them without going stale
 *  for the 10s roster poll. */
let statusCache: { at: number; key: string; value: AgentContainers[] } | null = null

export async function containerStatus(departments: string[]): Promise<AgentContainers[]> {
  const key = [...departments].sort().join(',')
  if (statusCache && statusCache.key === key && Date.now() - statusCache.at < 5_000) return statusCache.value
  const value = await containerStatusFresh(departments)
  statusCache = { at: Date.now(), key, value }
  return value
}

async function containerStatusFresh(departments: string[]): Promise<AgentContainers[]> {
  const { stdout } = await run('docker', ['ps', '-a', '--format', '{{json .}}'], { timeout: 20_000 })
  const all = stdout
    .split('\n')
    .filter(Boolean)
    .map((l) => JSON.parse(l) as { Names: string; State: string; Status: string })
  const byName = new Map(all.map((c) => [c.Names, { name: c.Names, state: c.State, status: c.Status, health: parseHealth(c.Status) }]))
  return departments.map((department) => {
    const a = byName.get(slotContainer(department, 'a'))
    const b = byName.get(slotContainer(department, 'b'))
    return { department, managed: (a?.state === 'running' ? a : b?.state === 'running' ? b : (a ?? b)) ?? null }
  })
}

/** Prune Hermes's bundled skill packs from an agent (best-effort). The image
 *  seeds note-tool skills (obsidian, notion, airtable, …) that compete with
 *  the Talaria toolkit and send agents flailing through vault hunts. The
 *  `.no-bundled-skills` marker stops re-seeding; --remove clears what's
 *  already on the volume. Talaria-managed skills (the mounted /opt/skills +
 *  /opt/dept-skills roots) are untouched — they're the ONLY skills an agent
 *  should carry. */
/** Bundled skill packs that CONFLICT with the Talaria toolkit — they pitch a
 *  parallel system of record (external note vaults, ungoverned email) and
 *  send agents flailing. Removed explicitly because the seed marks packs
 *  "user-modified", which opt-out --remove preserves. */
const CONFLICTING_SKILL_PACKS = [
  'note-taking', // obsidian — Talaria KB is the knowledgebase
  'productivity/notion',
  'productivity/airtable',
  'productivity/google-workspace', // Talaria's Google integration is confirm-send governed
  'email', // draft_email/read_recent_email govern mail through Talaria
]

export async function pruneBundledSkills(department: string, slot?: Slot): Promise<boolean> {
  const name = slotContainer(department, slot ?? (await activeSlot(department)))
  try {
    // Surgical: ONLY the conflict list goes — the rest of the bundled packs
    // (creative, data-science, software-development, …) are genuinely useful
    // and stay. Runs on every roll and fresh boot, so a re-seed that restores
    // a conflicting pack gets cut again on the next lifecycle event.
    const paths = CONFLICTING_SKILL_PACKS.map((p) => `/opt/data/skills/${p}`).join(' ')
    await run('docker', ['exec', name, 'sh', '-c', `rm -rf ${paths}`], { timeout: 30_000 })
    return true
  } catch {
    return false
  }
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
