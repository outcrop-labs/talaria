import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { stringify as stringifyYaml } from 'yaml'
import { requireUser } from '@/server/api-guard'
import { getRevision, listHistory, type InternalKind } from '@/server/internal-history'
import { listVersions } from '@/server/agent-defs'
import { ownsAgent, personalityOf } from '@/server/personal-agent'
import { getArtifact, guarded } from '@/server/artifacts'
import { getDoc, getSpace, effectiveDocPerms } from '@/server/kb'
import { canRead, listEditors } from '@/server/kb-perms'

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

/** The live item's read model, applied to its history. Fail closed. */
async function canReadSnapshotHistory(
  kind: InternalKind,
  ownerKey: string,
  user: { id: string; role: string; email: string | null; name: string | null },
): Promise<boolean> {
  const who = user.email ?? user.name
  try {
    switch (kind) {
      case 'artifact': {
        const a = await getArtifact(ownerKey)
        if (!a) return false
        return canRead(guarded(a), user.id, who, await listEditors('artifact', a.id))
      }
      case 'kb-doc': {
        const d = await getDoc(ownerKey)
        if (!d) return false
        const { perms, grants } = await effectiveDocPerms(d)
        return canRead(perms, user.id, who, grants)
      }
      case 'kb-space': {
        const sp = await getSpace(ownerKey)
        if (!sp) return false
        return canRead(sp, user.id, who, await listEditors('space', sp.id))
      }
      case 'memory':
        return user.role === 'admin' || (await ownsAgent(user.id, { defId: ownerKey }))
      case 'skill': {
        // ownerKey is "<slug>/<name>" — shared skills + org agents are
        // admin-editable surfaces; a personal assistant's skills are its
        // owner's business.
        const slug = ownerKey.split('/')[0] ?? ''
        return user.role === 'admin' || (await ownsAgent(user.id, { slug }))
      }
      case 'template':
        return user.role === 'admin'
    }
  } catch {
    return false
  }
  return false
}
const VERSION_KINDS = ['soul', 'config', 'personality'] as const
type VersionKind = (typeof VERSION_KINDS)[number]

const versionContent = (kind: VersionKind, v: { soul: string; config: unknown }): string =>
  kind === 'soul' ? v.soul : kind === 'config' ? stringifyYaml(v.config ?? {}) : (personalityOf(v.soul) ?? '')

export const Route = defineApi('/api/history', {
  GET: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
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

    // History serves FULL content — it must honor the same read model as
    // the live item, or it's a bypass of the entire permission system.
    const allowed = await canReadSnapshotHistory(kind as InternalKind, ownerKey, user)
    if (!allowed) return json({ error: 'forbidden' }, { status: 403 })

    if (rev) {
      const content = await getRevision(kind as InternalKind, ownerKey, rev)
      if (content === null) return json({ error: 'not found' }, { status: 404 })
      return json({ content })
    }
    return json({ revisions: await listHistory(kind as InternalKind, ownerKey) })
  },
})
