// Contacts MCP surface — tools Talaria AGENTS can call, governed in
// Manage → MCP exactly like any server: assign agents, narrow tool subsets,
// gate people. Calls dispatch in-process with this app's store.
import { defineAppMcp } from '@talaria/sdk/server'

interface Contact {
  name: string
  company?: string
  email?: string
  stage?: string
  notes?: string
}

export default defineAppMcp({
  tools: [
    {
      name: 'contacts_search',
      description: 'Search the CRM by name, company, email, or notes. Returns matching contacts with their stage.',
      inputSchema: {
        type: 'object',
        properties: { query: { type: 'string', description: 'Text to match (empty lists everyone)' } },
      },
      async handler(args, ctx) {
        const q = String(args.query ?? '').toLowerCase()
        const all = await ctx.store.list<Contact>('contacts', { limit: 500 })
        const hits = all.filter(
          (c) => !q || [c.data.name, c.data.company, c.data.email, c.data.notes].some((v) => v?.toLowerCase().includes(q)),
        )
        return hits.map((c) => ({ id: c.id, ...c.data, updatedAt: c.updatedAt }))
      },
    },
    {
      name: 'contacts_add',
      description: 'Add a contact to the CRM.',
      inputSchema: {
        type: 'object',
        properties: {
          name: { type: 'string' },
          company: { type: 'string' },
          email: { type: 'string' },
          stage: { type: 'string' },
          notes: { type: 'string' },
        },
        required: ['name'],
      },
      async handler(args, ctx) {
        const name = String(args.name ?? '').trim()
        if (!name) throw new Error('name is required')
        const doc = await ctx.store.insert('contacts', {
          name,
          company: String(args.company ?? ''),
          email: String(args.email ?? ''),
          stage: String(args.stage ?? 'lead'),
          notes: String(args.notes ?? ''),
          createdBy: `agent:${ctx.agent}`,
        })
        return { id: doc.id, ...doc.data }
      },
    },
    {
      name: 'contacts_note',
      description: 'Append a note to an existing contact (by id from contacts_search).',
      inputSchema: {
        type: 'object',
        properties: { id: { type: 'string' }, note: { type: 'string' } },
        required: ['id', 'note'],
      },
      async handler(args, ctx) {
        const doc = await ctx.store.get<Contact>('contacts', String(args.id))
        if (!doc) throw new Error('no such contact')
        const stamp = new Date().toISOString().slice(0, 10)
        const notes = `${doc.data.notes ? doc.data.notes + '\n' : ''}[${stamp} · ${ctx.agent}] ${String(args.note)}`
        await ctx.store.update('contacts', doc.id, { notes })
        return { ok: true, id: doc.id }
      },
    },
  ],
})
