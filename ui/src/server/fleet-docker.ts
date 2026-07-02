// Docker control for the managed fleet. Talaria drives `docker compose` on the
// rendered fleet/docker-compose.yml (interpolation env = the stack's .env, so
// per-agent keys/secrets stay in one gitignored place). Legacy containers (the
// old ai-* project) are start/stop-able too, which is how migration hands over.
import { execFile } from 'node:child_process'
import { promisify } from 'node:util'
import { join } from 'node:path'
import { FLEET_DIR, STACK_DIR } from './fleet-render'

const run = promisify(execFile)

// -p is mandatory: the stack's .env sets COMPOSE_PROJECT_NAME=ai, which would
// otherwise hijack the project and treat the ENTIRE legacy stack as orphans.
const composeArgs = (args: string[]) => [
  'compose',
  '-p',
  'talaria-fleet',
  '-f',
  join(FLEET_DIR(), 'docker-compose.yml'),
  '--env-file',
  join(STACK_DIR(), '.env'),
  ...args,
]

export async function fleetUp(department: string): Promise<string> {
  const { stderr } = await run('docker', composeArgs(['up', '-d', `agent-${department}`]), { timeout: 120_000 })
  return stderr.trim()
}

export async function fleetStop(department: string): Promise<string> {
  const { stderr } = await run('docker', composeArgs(['stop', `agent-${department}`]), { timeout: 60_000 })
  return stderr.trim()
}

/** Restart a managed agent so a re-rendered config.yaml takes effect. */
export async function fleetRestart(department: string): Promise<string> {
  const { stderr } = await run('docker', composeArgs(['restart', `agent-${department}`]), { timeout: 120_000 })
  return stderr.trim()
}

export async function fleetRemove(department: string): Promise<string> {
  const { stderr } = await run('docker', composeArgs(['rm', '-sf', `agent-${department}`]), { timeout: 60_000 })
  return stderr.trim()
}

export async function legacyControl(department: string, action: 'start' | 'stop'): Promise<void> {
  await run('docker', [action, `ai-agent-${department}-1`], { timeout: 60_000 })
}

export interface ContainerState {
  name: string
  state: string // running | exited | …
  status: string // human string incl. health, e.g. "Up 2 hours (healthy)"
}

export interface AgentContainers {
  department: string
  managed: ContainerState | null
  legacy: ContainerState | null
}

/** Container reality per department: the talaria-managed service and the legacy
 *  ai-project container, matched by name. */
export async function containerStatus(departments: string[]): Promise<AgentContainers[]> {
  const { stdout } = await run('docker', ['ps', '-a', '--format', '{{json .}}'], { timeout: 20_000 })
  const all = stdout
    .split('\n')
    .filter(Boolean)
    .map((l) => JSON.parse(l) as { Names: string; State: string; Status: string })
  const byName = new Map(all.map((c) => [c.Names, { name: c.Names, state: c.State, status: c.Status }]))
  return departments.map((department) => ({
    department,
    managed: byName.get(`talaria-fleet-agent-${department}-1`) ?? null,
    legacy: byName.get(`ai-agent-${department}-1`) ?? null,
  }))
}

/** Wait for the managed container to report healthy (or give up). */
export async function waitHealthy(department: string, timeoutMs = 120_000): Promise<boolean> {
  const name = `talaria-fleet-agent-${department}-1`
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
