import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { z } from 'zod'
import { getSessionUser } from '@/server/auth/session'
import { deleteDoc, getDoc, saveDoc, setOfficial } from '@/server/kb'
import { logAudit } from '@/server/audit'

const Patch = z.object({
  title: z.string().max(200).optional(),
  body: z.string().max(500_000).optional(),
  icon: z.string().max(16).nullish(),
  visibility: z.enum(['private', 'org', 'public']).optional(),
  parentId: z.string().uuid().nullish(),
  official: z.boolean().optional(),
})

// One KB doc. GET → full doc. PUT → save (versioned; official docs re-index).
// DELETE → remove. A private doc is only visible to its creator.
export const Route = createFileRoute('/api/kb/docs/$id')({
  server: {
    handlers: {
      GET: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const doc = await getDoc(params.id)
        if (!doc) return json({ error: 'not found' }, { status: 404 })
        const author = user.email ?? user.name
        if (doc.visibility === 'private' && doc.createdBy !== author) return json({ error: 'forbidden' }, { status: 403 })
        return json({ doc })
      },
      PUT: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const parsed = Patch.safeParse(await request.json().catch(() => null))
        if (!parsed.success) return json({ error: 'bad request' }, { status: 400 })
        const actor = user.email ?? user.name ?? 'user'
        let doc = await saveDoc(params.id, parsed.data, actor)
        if (!doc) return json({ error: 'not found' }, { status: 404 })
        if (parsed.data.official !== undefined && parsed.data.official !== doc.official) {
          doc = (await setOfficial(params.id, parsed.data.official, actor)) ?? doc
          void logAudit({ actor, action: parsed.data.official ? 'kb.officialize' : 'kb.deofficialize', targetType: 'kb-doc', targetId: params.id, targetLabel: doc.title })
        }
        return json({ doc })
      },
      DELETE: async ({ request, params }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        await deleteDoc(params.id)
        return json({ ok: true })
      },
    },
  },
})
