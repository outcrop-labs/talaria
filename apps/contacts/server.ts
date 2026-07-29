// Contacts app server — mounted by the host at /api/apps/contacts/*.
// The host has already authenticated the user and checked access; this file
// only decides what the app itself allows (e.g. wipe is admin-only).
import { defineAppServer, json, type AppRequestContext } from '@talaria/sdk/server'

interface Contact extends Record<string, unknown> {
  name: string
  company?: string
  email?: string
  stage?: string
  notes?: string
}

const DEFAULT_STAGES = ['lead', 'active', 'customer', 'dormant']

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
      const b = (await request.json().catch(() => ({}))) as Partial<Contact>
      if (!b.name?.trim()) return json({ error: 'name is required' }, { status: 400 })
      const doc = await store.insert('contacts', {
        name: b.name.trim(),
        company: b.company?.trim() ?? '',
        email: b.email?.trim() ?? '',
        stage: b.stage ?? (await getStages(ctx))[0] ?? 'lead',
        notes: b.notes ?? '',
        createdBy: ctx.user.name ?? ctx.user.email ?? ctx.user.id,
      })
      return json({ contact: doc })
    }

    const contactId = /^contacts\/([0-9a-f-]{36})$/.exec(path)?.[1]
    if (contactId && method === 'PATCH') {
      const b = (await request.json().catch(() => ({}))) as Record<string, unknown>
      const patch: Record<string, unknown> = {}
      for (const k of ['name', 'company', 'email', 'stage', 'notes'] as const) {
        if (typeof b[k] === 'string') patch[k] = b[k]
      }
      if (typeof patch.name === 'string' && !patch.name.trim()) return json({ error: 'name cannot be empty' }, { status: 400 })
      const doc = await store.update('contacts', contactId, patch)
      return doc ? json({ contact: doc }) : json({ error: 'not found' }, { status: 404 })
    }
    if (contactId && method === 'DELETE') {
      return (await store.remove('contacts', contactId)) ? json({ ok: true }) : json({ error: 'not found' }, { status: 404 })
    }

    if (path === 'config' && method === 'GET') return json({ stages: await getStages(ctx) })
    if (path === 'config' && method === 'PUT') {
      const b = (await request.json().catch(() => ({}))) as { stages?: unknown }
      const stages = Array.isArray(b.stages) ? b.stages.filter((s): s is string => typeof s === 'string' && !!s.trim()).slice(0, 12) : null
      if (!stages || stages.length === 0) return json({ error: 'stages must be a non-empty list' }, { status: 400 })
      const rows = await ctx.store.list('config', { limit: 1 })
      if (rows[0]) await store.update('config', rows[0].id, { stages })
      else await store.insert('config', { stages })
      return json({ stages })
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

    return json({ error: 'not found' }, { status: 404 })
  },
})
