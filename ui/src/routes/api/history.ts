import { createFileRoute } from '@tanstack/react-router'
import { json } from '@tanstack/react-start'
import { stringify as stringifyYaml } from 'yaml'
import { getSessionUser } from '@/server/auth/session'
import { getRevision, listHistory, type InternalKind } from '@/server/internal-history'
import { listVersions } from '@/server/agent-defs'
import { ownsAgent, personalityOf } from '@/server/personal-agent'

// Version history for agent internals, one API over two stores:
//   snapshot store (internal_versions): skill, memory, kb-doc, kb-space, artifact
//   agent versions (agent_versions):    soul, config, personality
//
//   GET /api/history?kind=skill&owner=<owner>&name=<name>   → revisions
//   GET /api/history?kind=memory&id=<defId>                 → revisions
//   GET /api/history?kind=soul|config|personality&id=<defId>
//   &rev=<id>                                              → that revision's content
// ownerKey is derived server-side (skill: "<owner>/<name>", the rest: an id) so
// the caller never constructs the storage key. Version-backed kinds are
// admin-or-owner: souls and configs are the agent's internals, not public.
const SNAPSHOT_KINDS = ['skill', 'memory', 'kb-doc', 'kb-space', 'artifact', 'template'] as const
const VERSION_KINDS = ['soul', 'config', 'personality'] as const
type VersionKind = (typeof VERSION_KINDS)[number]

const versionContent = (kind: VersionKind, v: { soul: string; config: unknown }): string =>
  kind === 'soul' ? v.soul : kind === 'config' ? stringifyYaml(v.config ?? {}) : (personalityOf(v.soul) ?? '')

export const Route = createFileRoute('/api/history')({
  server: {
    handlers: {
      GET: async ({ request }) => {
        const user = await getSessionUser(request)
        if (!user) return json({ error: 'unauthorized' }, { status: 401 })
        const q = new URL(request.url).searchParams
        const kind = q.get('kind') as InternalKind | VersionKind | null
        const rev = q.get('rev')

        if (VERSION_KINDS.includes(kind as VersionKind)) {
          const id = q.get('id')
          if (!id) return json({ error: 'missing id' }, { status: 400 })
          if (user.role !== 'admin' && !(await ownsAgent(user.id, { defId: id })))
            return json({ error: 'forbidden' }, { status: 403 })
          const versions = await listVersions(id)
          if (rev) {
            const v = versions.find((x) => x.id === rev)
            if (!v) return json({ error: 'not found' }, { status: 404 })
            return json({ content: versionContent(kind as VersionKind, v) })
          }
          return json({
            revisions: versions.slice(0, 50).map((v) => ({
              id: v.id,
              createdBy: v.createdBy,
              createdAt: v.createdAt,
              size: versionContent(kind as VersionKind, v).length,
              note: v.note,
              version: v.version,
            })),
          })
        }

        if (!SNAPSHOT_KINDS.includes(kind as InternalKind)) return json({ error: 'bad kind' }, { status: 400 })
        // skill keys on "<owner>/<name>"; the rest key on an id.
        const ownerKey =
          kind === 'skill'
            ? q.get('owner') && q.get('name')
              ? `${q.get('owner')}/${q.get('name')}`
              : null
            : q.get('id')
        if (!ownerKey) return json({ error: 'missing owner' }, { status: 400 })

        if (rev) {
          const content = await getRevision(kind as InternalKind, ownerKey, rev)
          if (content === null) return json({ error: 'not found' }, { status: 404 })
          return json({ content })
        }
        return json({ revisions: await listHistory(kind as InternalKind, ownerKey) })
      },
    },
  },
})
