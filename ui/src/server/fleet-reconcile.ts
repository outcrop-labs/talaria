// Bring the running fleet to the desired state in one shot: re-render every
// managed agent's config, then start any enabled managed agent whose container
// isn't running. Reboot survival is already handled by `restart: unless-stopped`
// on the generated services; this covers drift (an agent enabled/created while
// Talaria was down, a stopped container, a manifest change).
import { db } from './db/pg'
import { renderFleet } from './fleet-render'
import { containerStatus, fleetUp } from './fleet-docker'

export interface ReconcileResult {
  rendered: number
  started: string[]
  alreadyRunning: string[]
  warnings: string[]
}

export async function reconcileFleet(): Promise<ReconcileResult> {
  const sql = await db()
  const render = await renderFleet()

  const managed = (await sql`
    select department, display_name as "displayName" from agent_defs
    where managed and enabled order by slug
  `) as unknown as Array<{ department: string; displayName: string }>

  const states = managed.length ? await containerStatus(managed.map((m) => m.department)) : []
  const started: string[] = []
  const alreadyRunning: string[] = []
  const warnings = [...render.warnings]

  for (const m of managed) {
    const running = states.find((s) => s.department === m.department)?.managed?.state === 'running'
    if (running) {
      alreadyRunning.push(m.displayName)
      continue
    }
    try {
      await fleetUp(m.department)
      started.push(m.displayName)
    } catch (e) {
      warnings.push(`${m.displayName}: ${(e as Error).message}`)
    }
  }

  return { rendered: render.agents.length, started, alreadyRunning, warnings }
}
