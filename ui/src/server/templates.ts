// Ticket/plan templates: an org-wide library of markdown skeletons + prompt
// guidance. Boards bind the ticket templates they use (one default); agents may
// carry their own override ("Jax always writes eng tickets"). Resolution order,
// everywhere a template applies:
//   explicit pick → agent binding → board default → none (freeform).
import { db } from './db/pg'

export type TemplateKind = 'ticket' | 'plan'

export interface Template {
  id: string
  name: string
  kind: TemplateKind
  /** The markdown skeleton — sections the filled document must keep. */
  body: string
  /** Prompt guidance for the agent filling it (never shown on the ticket). */
  guidance: string
  createdBy: string | null
  updatedAt: string
}

const COLS = `id, name, kind, body, guidance, created_by as "createdBy", updated_at as "updatedAt"`

export async function listTemplates(kind?: TemplateKind): Promise<Template[]> {
  const sql = await db()
  const rows = kind
    ? await sql.unsafe(`select ${COLS} from templates where kind = $1 order by name asc`, [kind])
    : await sql.unsafe(`select ${COLS} from templates order by kind asc, name asc`)
  return rows as unknown as Template[]
}

export async function getTemplate(id: string): Promise<Template | null> {
  const sql = await db()
  const rows = (await sql.unsafe(`select ${COLS} from templates where id = $1`, [id])) as unknown as Template[]
  return rows[0] ?? null
}

export async function createTemplate(t: {
  name: string
  kind: TemplateKind
  body?: string
  guidance?: string
  createdBy: string
}): Promise<Template> {
  const sql = await db()
  const rows = (await sql`
    insert into templates (name, kind, body, guidance, created_by)
    values (${t.name}, ${t.kind}, ${t.body ?? ''}, ${t.guidance ?? ''}, ${t.createdBy})
    returning ${sql.unsafe(COLS)}
  `) as unknown as Template[]
  return rows[0]!
}

export async function updateTemplate(
  id: string,
  patch: { name?: string; body?: string; guidance?: string },
): Promise<Template | null> {
  const sql = await db()
  if (patch.name !== undefined) await sql`update templates set name = ${patch.name}, updated_at = now() where id = ${id}`
  if (patch.body !== undefined) await sql`update templates set body = ${patch.body}, updated_at = now() where id = ${id}`
  if (patch.guidance !== undefined)
    await sql`update templates set guidance = ${patch.guidance}, updated_at = now() where id = ${id}`
  return getTemplate(id)
}

export async function deleteTemplate(id: string): Promise<void> {
  const sql = await db()
  await sql`delete from templates where id = ${id}`
}

// ── Board bindings (ticket templates) ───────────────────────────────────────

export interface BoardTemplateBinding {
  templateId: string
  isDefault: boolean
}

export async function boardTemplates(boardId: string): Promise<BoardTemplateBinding[]> {
  const sql = await db()
  const rows = await sql`
    select template_id as "templateId", is_default as "isDefault"
    from board_templates where board_id = ${boardId}
  `
  return rows as unknown as BoardTemplateBinding[]
}

/** Replace a board's template set. `defaultId` must be in `templateIds`. */
export async function setBoardTemplates(boardId: string, templateIds: string[], defaultId: string | null): Promise<void> {
  const sql = await db()
  await sql.begin(async (tx) => {
    await tx`delete from board_templates where board_id = ${boardId}`
    for (const id of templateIds) {
      await tx`
        insert into board_templates (board_id, template_id, is_default)
        values (${boardId}, ${id}, ${id === defaultId})
        on conflict do nothing
      `
    }
  })
}

// ── Agent bindings ──────────────────────────────────────────────────────────

export async function agentTemplateBindings(
  agentModel: string,
): Promise<{ ticketTemplateId: string | null; planTemplateId: string | null }> {
  const sql = await db()
  const rows = (await sql`
    select ticket_template_id as "ticketTemplateId", plan_template_id as "planTemplateId"
    from agent_defs where model = ${agentModel}
  `) as unknown as Array<{ ticketTemplateId: string | null; planTemplateId: string | null }>
  return rows[0] ?? { ticketTemplateId: null, planTemplateId: null }
}

/** Bind/unbind an agent's default templates (null clears; undefined leaves). */
export async function setAgentTemplates(
  agentModel: string,
  patch: { ticketTemplateId?: string | null; planTemplateId?: string | null },
): Promise<void> {
  const sql = await db()
  if (patch.ticketTemplateId !== undefined)
    await sql`update agent_defs set ticket_template_id = ${patch.ticketTemplateId} where model = ${agentModel}`
  if (patch.planTemplateId !== undefined)
    await sql`update agent_defs set plan_template_id = ${patch.planTemplateId} where model = ${agentModel}`
}

// ── Resolution ──────────────────────────────────────────────────────────────

/** The template that applies in a context: explicit pick → the agent's own
 *  binding → the board's default (tickets) → none. Dead references (a deleted
 *  template) fall through to the next link. */
export async function resolveTemplate(
  kind: TemplateKind,
  ctx: { explicitId?: string | null; agentModel?: string | null; boardId?: string | null },
): Promise<Template | null> {
  if (ctx.explicitId) {
    const t = await getTemplate(ctx.explicitId)
    if (t && t.kind === kind) return t
  }
  if (ctx.agentModel) {
    const b = await agentTemplateBindings(ctx.agentModel)
    const id = kind === 'ticket' ? b.ticketTemplateId : b.planTemplateId
    if (id) {
      const t = await getTemplate(id)
      if (t && t.kind === kind) return t
    }
  }
  if (kind === 'ticket' && ctx.boardId) {
    const def = (await boardTemplates(ctx.boardId)).find((b) => b.isDefault)
    if (def) {
      const t = await getTemplate(def.templateId)
      if (t && t.kind === kind) return t
    }
  }
  return null
}

/** The prompt block a template contributes to an agent's instructions. */
export function templatePrompt(t: Template, what: 'ticket descriptions' | 'the plan document'): string {
  const parts = [
    `Format ${what} on this template — keep its headings/section structure, fill every section (use "n/a" only when truly inapplicable):\n<<<\n${t.body}\n>>>`,
  ]
  if (t.guidance.trim()) parts.push(`Template guidance: ${t.guidance.trim()}`)
  return parts.join('\n')
}
