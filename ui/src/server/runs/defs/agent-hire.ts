// THE AGENT-HIRE RUN — "create an agent" as durable work, on the same runtime
// research and plan-drafts already run on.
//
// WHY A RUN AT ALL, when the route used to do this synchronously in one POST:
// hiring is not a row write, it is a BOOT — render the fleet config, `docker
// compose up`, and wait out a healthcheck window that runs to two minutes on a
// cold pull. Inside one POST that is a promise to stay on the line: the modal
// could not close, a proxy could kill the request long before waitHealthy
// finished, and an agent whose request died was still created server-side,
// visible only after a refresh nobody knew they needed. A run is a row. The
// modal closes the moment the intent is recorded, the roster shows the hire
// working through its phases, and a server restart mid-boot re-enters from the
// last stage that finished.
//
// THREE STAGES, ONE STEP FUNCTION (the engine's contract is a single `step`
// re-entered with a checkpoint; stages are the checkpoint's value):
//   create → key + def row + v1 config + starter skills + audit
//   render → write the fleet compose/config files
//   boot   → up + waitHealthy, only when `start` was asked for
//
// RESUME RULES. `create` is the one stage that is not naturally idempotent —
// createAgent refuses a taken slug — so a driver that dies between the def row
// and the checkpoint write would, on reclaim, error against its own work.
// The real deps therefore treat "already exists" as the resume signal and hand
// back the existing def: the work is done, not failed. `render` and `boot` are
// idempotent by nature (they rewrite/re-up the same files).
import { registerRun, type RunDefinition, type StepResult } from '../define'
import type { Authority } from '../../approvals'
import { createAgent } from '../../fleet-create'
import type { AgentDef } from '../../agent-defs'
import { writeSkill } from '../../agent-skills'
import { logAudit } from '../../audit'
import { renderFleet } from '../../fleet-render'
import { fleetUp, waitHealthy } from '../../fleet-docker'
import { db } from '../../db/pg'

export interface AgentHireInput {
  slug: string
  department: string
  displayName: string
  role: string | null
  /** Clone this agent's config; null for the platform defaults. */
  templateId: string | null
  /** Override the starter-soul scaffold (e.g. an AI-designed soul). */
  soul: string | null
  /** Starter skills written after the def exists. */
  skills: Array<{ name: string; content: string }>
  /** Render + up + wait for health, or just write the def. */
  start: boolean
  /** Audit actor — the email/name of the admin who clicked Create. */
  actor: string
}

export interface AgentHireCheckpoint {
  defId: string | null
  stage: 'create' | 'render' | 'boot'
  /** Warnings the render stage saw, carried to the result — boot never
   *  re-renders to recover them. */
  warnings: string[]
}

export interface AgentHireResult {
  defId: string
  /** Undefined when `start` was not asked for; otherwise the healthcheck's
   *  answer — false means created but not healthy, which is a warning the
   *  roster already knows how to show, not a failed hire. */
  healthy: boolean | undefined
  /** Warnings from the fleet render, surfaced verbatim. */
  warnings: string[]
}

export interface AgentHireDeps {
  create: (input: AgentHireInput) => Promise<{ def: AgentDef; keyCreated: boolean }>
  writeSkills: (slug: string, skills: AgentHireInput['skills'], actor: string) => Promise<void>
  audit: (def: AgentDef, actor: string) => void
  render: () => Promise<{ warnings: string[] }>
  /** fleetUp answers with the compose service it brought up; the run doesn't
   *  read it, so the dep just promises SOMETHING. */
  up: (department: string) => Promise<unknown>
  waitHealthy: (department: string) => Promise<boolean>
}

/** The "already exists" resume: a re-entered `create` stage finds the def the
 *  previous driver already wrote and hands it back instead of erroring. The
 *  slug is the identity a person chose; two hires racing on the same slug
 *  still collide honestly at the route's pre-check. */
async function createOrResume(input: AgentHireInput): Promise<{ def: AgentDef; keyCreated: boolean }> {
  try {
    return await createAgent({
      slug: input.slug,
      department: input.department,
      displayName: input.displayName,
      role: input.role,
      ...(input.templateId ? { templateId: input.templateId } : {}),
      ...(input.soul ? { soul: input.soul } : {}),
      createdBy: input.actor,
    })
  } catch (e) {
    if (!(e instanceof Error) || !e.message.includes('already exists')) throw e
    const sql = await db()
    const rows = (await sql`select * from agent_defs where slug = ${input.slug}`) as unknown as AgentDef[]
    const def = rows[0]
    if (!def) throw e
    return { def, keyCreated: false }
  }
}

export const REAL_AGENT_HIRE_DEPS: AgentHireDeps = {
  create: createOrResume,
  writeSkills: async (slug, skills, actor) => {
    for (const s of skills) await writeSkill(slug, s.name, s.content, actor).catch(() => {})
  },
  audit: (def, actor) => {
    void logAudit({
      actor,
      action: 'agent.create',
      targetType: 'agent',
      targetId: def.id,
      targetLabel: def.displayName,
      after: { slug: def.slug, department: def.department },
    })
  },
  render: () => renderFleet(),
  up: (department) => fleetUp(department),
  waitHealthy: (department) => waitHealthy(department),
}

export function makeAgentHireRun(deps: AgentHireDeps): RunDefinition<AgentHireInput, AgentHireCheckpoint> {
  return {
    kind: 'agent-hire',
    label: 'Hire agent',
    // Boot is the long stage: a cold pull plus the healthcheck window is
    // waitHealthy's own two-minute ceiling, and the up before it can take its
    // time building. The step ceiling clears the worst case rather than the
    // typical one; a step that blows THIS is filed as an error, not retried,
    // because it is probably still running.
    maxStepMs: 10 * 60_000,
    // Who may watch and be told: the admin who clicked Create (the surface
    // that lists hires is admin-gated anyway, same as the roster it lands in).
    audience: (run): Authority => (run.ownerUserId ? { by: 'user', userIds: [run.ownerUserId] } : { by: 'admin' }),
    step: async (ctx): Promise<StepResult<AgentHireCheckpoint>> => {
      const input = ctx.input
      const cp: AgentHireCheckpoint = ctx.checkpoint ?? { defId: null, stage: 'create', warnings: [] }

      if (cp.stage === 'create') {
        const { def } = await deps.create(input)
        await deps.writeSkills(def.slug, input.skills, input.actor)
        deps.audit(def, input.actor)
        ctx.log(`hiring ${def.displayName}: gateway key, config v1${input.skills.length ? `, ${input.skills.length} starter skill${input.skills.length === 1 ? '' : 's'}` : ''}`)
        return { kind: 'next', checkpoint: { defId: def.id, stage: 'render', warnings: [] }, phase: 'rendering the fleet config' }
      }

      // render or boot — both need the def id create wrote. A checkpoint here
      // without one is a corrupt row; the honest recovery is to run create
      // again, whose already-exists path makes that a read, not a duplicate.
      const defId = cp.defId
      if (!defId) return { kind: 'next', checkpoint: { defId: null, stage: 'create', warnings: cp.warnings } }

      if (cp.stage === 'render') {
        const render = await deps.render()
        if (!input.start) {
          for (const w of render.warnings) ctx.log(w)
          return { kind: 'done', result: { defId, healthy: undefined, warnings: render.warnings } satisfies AgentHireResult }
        }
        ctx.log(render.warnings.length ? `rendered with ${render.warnings.length} warning${render.warnings.length === 1 ? '' : 's'}` : 'fleet config rendered')
        return { kind: 'next', checkpoint: { ...cp, stage: 'boot', warnings: render.warnings }, phase: 'starting the container' }
      }

      // boot
      await deps.up(input.department)
      const healthy = await deps.waitHealthy(input.department)
      ctx.log(healthy ? `${input.displayName} is up and healthy` : `${input.displayName} created, but the container is not healthy yet`)
      return { kind: 'done', result: { defId, healthy, warnings: cp.warnings } satisfies AgentHireResult }
    },
  }
}

export const agentHireRun = registerRun(makeAgentHireRun(REAL_AGENT_HIRE_DEPS))
