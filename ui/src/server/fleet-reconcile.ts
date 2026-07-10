// Bring the running fleet to the desired state in one shot: re-render every
// managed agent's config, then start any enabled managed agent whose container
// isn't running. Reboot survival is already handled by `restart: unless-stopped`
// on the generated services; this covers drift (an agent enabled/created while
// Talaria was down, a stopped container, a manifest change).
import { db } from './db/pg'
import { nextFreePort, renderFleet } from './fleet-render'
import { containerStatus, fleetRemove, fleetUp, pruneBundledSkills, removeContainerByName, slotContainer, waitHealthy, type Slot } from './fleet-docker'

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
      // New containers get the bundled note-tool skills stripped once healthy
      // — toolkit-first is the default from first boot. Detached: reconcile
      // must not block on health.
      void waitHealthy(m.department).then((ok) => ok && pruneBundledSkills(m.department)).catch(() => {})
    } catch (e) {
      warnings.push(`${m.displayName}: ${(e as Error).message}`)
    }
  }

  return { rendered: render.agents.length, started, alreadyRunning, warnings }
}

const ROLL_DRAIN_MS = () => Math.max(0, Number(process.env.TALARIA_ROLL_DRAIN_SECONDS ?? 45)) * 1000

/** Zero-downtime replacement of one agent's container. The incoming slot comes
 *  up on a FRESH port while the old one keeps serving; only after the newcomer
 *  is healthy does the manifest cut over (proxyChat re-reads it per call, so
 *  new turns route instantly); the old container gets a drain window to finish
 *  in-flight replies, then retires. A newcomer that never gets healthy is
 *  discarded — the old container never stopped serving. */
export async function rollAgent(department: string): Promise<{ ok: boolean; error?: string }> {
  const sql = await db()
  const rows = (await sql`
    select slug, display_name as "displayName", active_slot as "activeSlot"
    from agent_defs where department = ${department} and managed and enabled
  `) as unknown as Array<{ slug: string; displayName: string; activeSlot: string }>
  const def = rows[0]
  if (!def) return { ok: false, error: `no managed agent in department "${department}"` }
  const oldSlot: Slot = def.activeSlot === 'b' ? 'b' : 'a'
  const newSlot: Slot = oldSlot === 'a' ? 'b' : 'a'
  const newPort = await nextFreePort()

  // 1. Overlay render: both slots in the compose file; manifest still old.
  await renderFleet({ roll: { slug: def.slug, slot: newSlot, port: newPort } })
  // 2. Bring the incoming slot up and wait for real health.
  await fleetUp(department, newSlot)
  if (!(await waitHealthy(department, newSlot))) {
    await fleetRemove(department, newSlot).catch(() => {})
    await renderFleet() // back to steady state; the old container never blinked
    return { ok: false, error: `${def.displayName}: replacement never became healthy — kept the old container` }
  }
  // Toolkit-first by default: strip the image's bundled note-tool skills the
  // moment the newcomer is healthy (Talaria-managed skills are untouched).
  await pruneBundledSkills(department, newSlot).catch(() => {})
  // 3. Cutover: incoming slot becomes active (new port), manifest re-renders.
  await sql`update agent_defs set active_slot = ${newSlot}, gateway_port = ${newPort} where slug = ${def.slug}`
  await renderFleet()
  // 4. Drain in-flight replies on the old container, then retire it.
  await new Promise((r) => setTimeout(r, ROLL_DRAIN_MS()))
  await removeContainerByName(slotContainer(department, oldSlot)).catch(() => {})
  return { ok: true }
}

/** Propagate an identity-level change (e.g. the org profile) to the live
 *  fleet: re-render every managed soul, then ROLL running agents one at a time
 *  so nobody's conversation ever hits a dead container. Agents someone
 *  deliberately stopped stay stopped (they read the new render on next start). */
export async function rollRunningAgents(): Promise<{ rolled: string[]; warnings: string[] }> {
  const sql = await db()
  const render = await renderFleet()
  const managed = (await sql`
    select department, display_name as "displayName" from agent_defs
    where managed and enabled order by slug
  `) as unknown as Array<{ department: string; displayName: string }>
  const states = managed.length ? await containerStatus(managed.map((m) => m.department)) : []
  const rolled: string[] = []
  const warnings = [...render.warnings]
  for (const m of managed) {
    if (states.find((s) => s.department === m.department)?.managed?.state !== 'running') continue
    const r = await rollAgent(m.department).catch((e: Error) => ({ ok: false, error: e.message }))
    if (r.ok) rolled.push(m.displayName)
    else warnings.push(r.error ?? `${m.displayName}: roll failed`)
  }
  return { rolled, warnings }
}
