import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { getSessionUser } from '@/server/auth/session'
import { getRevision, listHistory, type InternalKind } from '@/server/internal-history'

// Version history for agent internals.
//   GET /api/history?kind=skill&owner=<owner>&name=<name>   → revisions
//   GET /api/history?kind=memory&id=<defId>                 → revisions
//   …&rev=<id>                                              → that revision's content
// ownerKey is derived server-side (skill: "<owner>/<name>", memory: defId) so
// the caller never constructs the storage key.
export const Route = createFileRoute('/api/history')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const q = new URL(request.url).searchParams
        const kind = q.get('kind') as InternalKind | null
        if (kind !== 'skill' && kind !== 'memory' && kind !== 'kb-doc' && kind !== 'kb-space' && kind !== 'artifact')
          return json({ error: 'bad kind' }, { status: 400 })
        // skill keys on "<owner>/<name>"; the rest key on an id.
        const ownerKey =
          kind === 'skill'
            ? q.get('owner') && q.get('name')
              ? `${q.get('owner')}/${q.get('name')}`
              : null
            : q.get('id')
        if (!ownerKey) return json({ error: 'missing owner' }, { status: 400 })

        const rev = q.get('rev')
        if (rev) {
          const content = await getRevision(kind, ownerKey, rev)
          if (content === null) return json({ error: 'not found' }, { status: 404 })
          return json({ content })
        }
        return json({ revisions: await listHistory(kind, ownerKey) })
      },
    },
  },
})
