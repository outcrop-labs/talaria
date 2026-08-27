// Contacts app server — mounted by the host at /api/apps/contacts/*.
// The host has already authenticated the user and checked access; this file
// only decides what the app itself allows (e.g. wipe is admin-only).
import { defineAppServer, json, parseBody, z, type AppRequestContext } from '@talaria/sdk/server'

interface Contact extends Record<string, unknown> {
  name: string
  company?: string
  email?: string
  stage?: string
  notes?: string
}

const DEFAULT_STAGES = ['lead', 'active', 'customer', 'dormant']

// The same door the host's routes use: zod in, validated data or the
// standard 400 out. The `{ error }` param keeps each field's missing-key
// message identical to its bad-value message ("name is required", not
// "expected string, received undefined").
const CreateBody = z.object({
  name: z.string({ error: 'name is required' }).trim().min(1, 'name is required'),
  company: z.string().trim().optional(),
  email: z.string().trim().optional(),
  stage: z.string().optional(),
  notes: z.string().optional(),
})
const PatchBody = z.object({
  name: z.string({ error: 'name cannot be empty' }).trim().min(1, 'name cannot be empty').optional(),
  company: z.string().optional(),
  email: z.string().optional(),
  stage: z.string().optional(),
  notes: z.string().optional(),
})
const ConfigBody = z.object({
  stages: z.array(z.string().trim().min(1, 'stages must be a non-empty list')).min(1, 'stages must be a non-empty list').max(12),
})

const getStages = async (ctx: AppRequestContext): Promise<string[]> => {
  const rows = await ctx.store.list<{ stages?: string[] }>('config', { limit: 1 })
  return rows[0]?.data.stages ?? DEFAULT_STAGES
}

export default defineAppServer({
  async fetch(request, ctx) {
    const { path, store } = ctx
    const method = request.method

    if (path === 'contacts' && method === 'GET') {
      const q = ctx.url.searchParams.get('q')?.toLowerCase() ?? ''
      const all = await store.list<Contact>('contacts', { limit: 500 })
      const hit = (c: Contact) =>
        !q || [c.name, c.company, c.email, c.notes].some((v) => v?.toLowerCase().includes(q))
      return json({ contacts: all.filter((d) => hit(d.data)) })
    }

    if (path === 'contacts' && method === 'POST') {
      const body = await parseBody(request, CreateBody)
      if (body instanceof Response) return body
      const doc = await store.insert('contacts', {
        name: body.name,
        company: body.company ?? '',
        email: body.email ?? '',
        stage: body.stage ?? (await getStages(ctx))[0] ?? 'lead',
        notes: body.notes ?? '',
        createdBy: ctx.user.name ?? ctx.user.email ?? ctx.user.id,
      })
      return json({ contact: doc })
    }

    const contactId = /^contacts\/([0-9a-f-]{36})$/.exec(path)?.[1]
    if (contactId && method === 'PATCH') {
      const body = await parseBody(request, PatchBody)
      if (body instanceof Response) return body
      const doc = await store.update('contacts', contactId, body)
      return doc ? json({ contact: doc }) : json({ error: 'not found' }, { status: 404 })
    }
    if (contactId && method === 'DELETE') {
      return (await store.remove('contacts', contactId)) ? json({ ok: true }) : json({ error: 'not found' }, { status: 404 })
    }

    if (path === 'config' && method === 'GET') return json({ stages: await getStages(ctx) })
    if (path === 'config' && method === 'PUT') {
      const body = await parseBody(request, ConfigBody)
      if (body instanceof Response) return body
      const rows = await ctx.store.list('config', { limit: 1 })
      if (rows[0]) await store.update('config', rows[0].id, { stages: body.stages })
      else await store.insert('config', { stages: body.stages })
      return json({ stages: body.stages })
    }

    if (path === 'stats' && method === 'GET') {
      const all = await store.list<Contact>('contacts', { limit: 1000 })
      const byStage: Record<string, number> = {}
      for (const c of all) byStage[c.data.stage ?? '—'] = (byStage[c.data.stage ?? '—'] ?? 0) + 1
      return json({ total: all.length, byStage })
    }

    if (path === 'wipe' && method === 'POST') {
      if (ctx.user.role !== 'admin') return json({ error: 'admins only' }, { status: 403 })
      await store.wipe()
      return json({ ok: true })
    }

    // Known path, wrong verb → the host's 405 + allow (not a 404, which
    // would tell a well-meaning client the endpoint doesn't exist).
    const ALLOWED: Record<string, string> = {
      contacts: 'GET, POST',
      config: 'GET, PUT',
      stats: 'GET',
      wipe: 'POST',
    }
    const allow = contactId ? 'PATCH, DELETE' : ALLOWED[path]
    return allow
      ? json({ error: 'method not allowed' }, { status: 405, headers: { allow } })
      : json({ error: 'not found' }, { status: 404 })
  },
})
