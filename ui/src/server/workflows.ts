// Task workflows — the hook layer between "an agent got a ticket" and "the
// agent works it the right way". A workflow is classified ONTO a task by its
// match rules (labels, boards, title/description keywords); everything that
// matches rides along with the work delivery (dispatch, heartbeat, get_ticket):
//   skills        Hermes skills that ARE the flow — the content lives in the
//                 skill library the agents already mount (/opt/skills +
//                 /opt/dept-skills); the workflow only names them
//   toolkits      declared MCP servers/tool subsets the work expects (granted
//                 through the MCP registry; declared here so the agent and
//                 the admin both see what the workflow assumes)
//   env           RESERVED: sandbox/runtime profile for the custom-runtime
//                 layer (dev containers, design pipelines) — schema now,
//                 execution later. Profiles will attach to agents/departments
//                 via chassis overlays; workflows only ROUTE work there.
import { db } from './db/pg'
import type { Task } from '@/lib/task-const'

export interface WorkflowMatch {
  labels?: string[]
  boards?: string[]
  keywords?: string[]
}

export interface TaskWorkflow {
  id: string
  name: string
  description: string
  enabled: boolean
  match: WorkflowMatch
  skills: string[]
  toolkits: Array<{ server: string; tools?: string[] }>
  env: Record<string, unknown>
  position: number
}

const ROW = `id, name, description, enabled, match, skills, toolkits, env, position`

export async function listWorkflows(): Promise<TaskWorkflow[]> {
  const sql = await db()
  return (await sql.unsafe(`select ${ROW} from task_workflows order by position, created_at`)) as unknown as TaskWorkflow[]
}

export async function createWorkflow(input: {
  name: string
  description?: string
  match?: WorkflowMatch
  skills?: string[]
  toolkits?: Array<{ server: string; tools?: string[] }>
  createdBy: string
}): Promise<TaskWorkflow> {
  const sql = await db()
  const rows = (await sql`
    insert into task_workflows (name, description, match, skills, toolkits, created_by, position)
    values (${input.name.trim()}, ${input.description ?? ''}, ${sql.json((input.match ?? {}) as Record<string, string[]>)}, ${sql.json(input.skills ?? [])},
            ${sql.json(input.toolkits ?? [])}, ${input.createdBy},
            coalesce((select max(position) + 1 from task_workflows), 0))
    returning ${sql.unsafe(ROW)}
  `) as unknown as TaskWorkflow[]
  return rows[0]!
}

export async function updateWorkflow(
  id: string,
  patch: {
    name?: string
    description?: string
    enabled?: boolean
    match?: WorkflowMatch
    skills?: string[]
    toolkits?: Array<{ server: string; tools?: string[] }>
  },
): Promise<void> {
  const sql = await db()
  if (patch.name !== undefined) await sql`update task_workflows set name = ${patch.name.trim()}, updated_at = now() where id = ${id}`
  if (patch.description !== undefined) await sql`update task_workflows set description = ${patch.description}, updated_at = now() where id = ${id}`
  if (patch.enabled !== undefined) await sql`update task_workflows set enabled = ${patch.enabled}, updated_at = now() where id = ${id}`
  if (patch.match !== undefined) await sql`update task_workflows set match = ${sql.json(patch.match as Record<string, string[]>)}, updated_at = now() where id = ${id}`
  if (patch.skills !== undefined) await sql`update task_workflows set skills = ${sql.json(patch.skills)}, updated_at = now() where id = ${id}`
  if (patch.toolkits !== undefined) await sql`update task_workflows set toolkits = ${sql.json(patch.toolkits)}, updated_at = now() where id = ${id}`
}

export async function deleteWorkflow(id: string): Promise<void> {
  const sql = await db()
  await sql`delete from task_workflows where id = ${id}`
}

/** Classify a task: every ENABLED workflow whose rules match. Rules OR within
 *  a facet, AND across facets a workflow actually sets; a workflow with no
 *  rules matches nothing (explicit > accidental catch-all). */
export function matchWorkflow(
  h: TaskWorkflow,
  t: Pick<Task, 'title' | 'description' | 'tags' | 'boardId'>,
): boolean {
  if (!h.enabled) return false
  const m = h.match ?? {}
  const facets: boolean[] = []
  if (m.labels?.length) facets.push(m.labels.some((l) => t.tags.includes(l)))
  if (m.boards?.length) facets.push(m.boards.includes(t.boardId))
  if (m.keywords?.length) {
    const hay = `${t.title}\n${t.description ?? ''}`.toLowerCase()
    facets.push(m.keywords.some((k) => hay.includes(k.toLowerCase())))
  }
  return facets.length > 0 && facets.every(Boolean)
}

export interface WorkflowDelivery {
  name: string
  skills: string[]
  toolkits: Array<{ server: string; tools?: string[] }>
}

/** The workflow payload delivered WITH the work (dispatch + heartbeat + get_ticket). */
export async function workflowsForTask(
  t: Pick<Task, 'title' | 'description' | 'tags' | 'boardId'>,
): Promise<WorkflowDelivery[]> {
  const all = await listWorkflows()
  return all
    .filter((h) => matchWorkflow(h, t))
    .map((h) => ({ name: h.name, skills: h.skills, toolkits: h.toolkits }))
}
