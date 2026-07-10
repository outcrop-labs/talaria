// The RAG collection registry. Talaria spins up as many collections as needed;
// two are auto-provisioned and undeletable:
//   activity  — the ambient workspace index (chats/channels/plans/research/…),
//               a retrieval TOOL agents call on demand (never auto-loaded)
//   org-kb    — the curated knowledgebase; grounds agents by default
// Others are custom (departmental etc.), bound to users/agents/groups.
import { db } from '../db/pg'
import { embedDim } from './embed'
import { ensureCollection, deleteCollection } from './qdrant'

export type CollectionKind = 'activity' | 'org-kb' | 'custom' | 'personal'

export interface RagCollection {
  id: string
  name: string
  kind: CollectionKind
  qdrantName: string
  description: string | null
  auto: boolean
  createdBy: string | null
  createdAt: string
}

export interface AccessBinding {
  principalType: 'all' | 'user' | 'agent' | 'team'
  principalId: string | null
}

const AUTO: Array<{ name: string; kind: CollectionKind; qdrantName: string; description: string }> = [
  { name: 'Workspace activity', kind: 'activity', qdrantName: 'talaria_activity', description: 'Ambient index of chats, channels, plans, research, and ticket discussion. Searched on demand.' },
  { name: 'Organization knowledge', kind: 'org-kb', qdrantName: 'talaria_org_kb', description: 'The curated knowledgebase — official docs and artifacts. Grounds agents by default.' },
]

/** Create the two auto collections + their Qdrant collections if missing. */
export async function ensureAutoCollections(): Promise<void> {
  const sql = await db()
  const dim = await embedDim().catch(() => 1024) // Qwen3-Embedding-0.6B
  for (const a of AUTO) {
    await ensureCollection(a.qdrantName, dim).catch(() => {})
    await sql`
      insert into rag_collections (name, kind, qdrant_name, description, auto, embed_dim)
      values (${a.name}, ${a.kind}, ${a.qdrantName}, ${a.description}, true, ${dim})
      on conflict (qdrant_name) do nothing
    `
  }
}

export async function listCollections(): Promise<Array<RagCollection & { bindings: AccessBinding[] }>> {
  const sql = await db()
  const cols = (await sql`
    select id, name, kind, qdrant_name as "qdrantName", description, auto,
           created_by as "createdBy", created_at as "createdAt"
    from rag_collections where kind <> 'personal' order by auto desc, name asc
  `) as unknown as RagCollection[]
  const access = (await sql`
    select collection_id as "collectionId", principal_type as "principalType", principal_id as "principalId"
    from rag_collection_access
  `) as unknown as Array<AccessBinding & { collectionId: string }>
  return cols.map((c) => ({ ...c, bindings: access.filter((a) => a.collectionId === c.id) }))
}

export async function getCollection(id: string): Promise<RagCollection | null> {
  const sql = await db()
  const rows = (await sql`
    select id, name, kind, qdrant_name as "qdrantName", description, auto,
           created_by as "createdBy", created_at as "createdAt"
    from rag_collections where id = ${id}
  `) as unknown as RagCollection[]
  return rows[0] ?? null
}

const slugify = (s: string) => 'talaria_' + s.toLowerCase().replace(/[^a-z0-9]+/g, '_').replace(/^_+|_+$/g, '').slice(0, 40)

export async function createCollection(input: {
  name: string
  description?: string
  createdBy: string
  bindings?: AccessBinding[]
}): Promise<RagCollection> {
  const sql = await db()
  const dim = await embedDim()
  let qdrantName = slugify(input.name)
  // Avoid a name clash with an existing collection.
  const clash = await sql`select 1 from rag_collections where qdrant_name = ${qdrantName}`
  if (clash.length) qdrantName = `${qdrantName}_${Date.now().toString(36)}`
  await ensureCollection(qdrantName, dim)
  const rows = (await sql`
    insert into rag_collections (name, kind, qdrant_name, description, auto, embed_dim, created_by)
    values (${input.name}, 'custom', ${qdrantName}, ${input.description ?? null}, false, ${dim}, ${input.createdBy})
    returning id, name, kind, qdrant_name as "qdrantName", description, auto, created_by as "createdBy", created_at as "createdAt"
  `) as unknown as RagCollection[]
  const col = rows[0]!
  await setBindings(col.id, input.bindings ?? [{ principalType: 'all', principalId: null }])
  return col
}

/** The user's personal RAG collection, if they have one. */
export async function personalCollectionFor(userId: string): Promise<RagCollection | null> {
  const sql = await db()
  const rows = (await sql`
    select id, name, kind, qdrant_name as "qdrantName", description, auto,
           created_by as "createdBy", created_at as "createdAt"
    from rag_collections where kind = 'personal' and owner_user_id = ${userId} limit 1
  `) as unknown as RagCollection[]
  return rows[0] ?? null
}

/** Create (or return) a user's personal RAG collection, bound to them and —
 *  when given — their personal agent. Their private KB docs sync here; nobody
 *  else is bound, so nobody else can retrieve from it. */
export async function ensurePersonalCollection(userId: string, opts: { name?: string; agentModel?: string } = {}): Promise<RagCollection> {
  const existing = await personalCollectionFor(userId)
  if (existing) {
    if (opts.agentModel) await addBinding(existing.id, { principalType: 'agent', principalId: opts.agentModel })
    return existing
  }
  const sql = await db()
  const dim = await embedDim()
  const qdrantName = `talaria_personal_${userId.replace(/-/g, '').slice(0, 24)}`
  await ensureCollection(qdrantName, dim)
  const rows = (await sql`
    insert into rag_collections (name, kind, qdrant_name, description, auto, embed_dim, created_by, owner_user_id)
    values (${opts.name ?? 'My knowledge'}, 'personal', ${qdrantName}, 'Your private docs — visible only to you and your personal assistant.', false, ${dim}, ${userId}, ${userId})
    returning id, name, kind, qdrant_name as "qdrantName", description, auto, created_by as "createdBy", created_at as "createdAt"
  `) as unknown as RagCollection[]
  const col = rows[0]!
  const bindings: AccessBinding[] = [{ principalType: 'user', principalId: userId }]
  if (opts.agentModel) bindings.push({ principalType: 'agent', principalId: opts.agentModel })
  await setBindings(col.id, bindings)
  return col
}

/** Add a single access binding without disturbing the others. */
async function addBinding(collectionId: string, binding: AccessBinding): Promise<void> {
  const sql = await db()
  await sql`
    insert into rag_collection_access (collection_id, principal_type, principal_id)
    values (${collectionId}, ${binding.principalType}, ${binding.principalId ?? null})
    on conflict do nothing
  `
}

export async function deleteCollectionById(id: string): Promise<{ ok: boolean; error?: string }> {
  const col = await getCollection(id)
  if (!col) return { ok: true }
  if (col.auto) return { ok: false, error: 'auto collections cannot be deleted' }
  const sql = await db()
  await deleteCollection(col.qdrantName)
  await sql`delete from rag_collections where id = ${id}`
  return { ok: true }
}

export async function setBindings(collectionId: string, bindings: AccessBinding[]): Promise<void> {
  const sql = await db()
  await sql.begin(async (tx) => {
    await tx`delete from rag_collection_access where collection_id = ${collectionId}`
    for (const b of bindings) {
      await tx`
        insert into rag_collection_access (collection_id, principal_type, principal_id)
        values (${collectionId}, ${b.principalType}, ${b.principalId ?? null})
        on conflict do nothing
      `
    }
  })
}

/** The collections a principal may search: 'all'-bound ones + those bound to
 *  this user or agent. Org-kb is always readable (it grounds everyone). */
export async function collectionsForPrincipal(principal: {
  userId?: string
  agentModel?: string
}): Promise<RagCollection[]> {
  const sql = await db()
  // Empty-string sentinels never match a real user id / agent model.
  const uid = principal.userId ?? ''
  const agent = principal.agentModel ?? ''
  const rows = (await sql`
    select distinct c.id, c.name, c.kind, c.qdrant_name as "qdrantName", c.description, c.auto,
           c.created_by as "createdBy", c.created_at as "createdAt"
    from rag_collections c
    left join rag_collection_access a on a.collection_id = c.id
    where c.auto = true
       or a.principal_type = 'all'
       or (a.principal_type = 'user' and ${uid} <> '' and a.principal_id = ${uid})
       or (a.principal_type = 'agent' and ${agent} <> '' and a.principal_id = ${agent})
       or (a.principal_type = 'team' and ${uid} <> '' and exists (
             select 1 from team_members tm where tm.team_id::text = a.principal_id and tm.user_id = ${uid}
           ))
    order by c.name asc
  `) as unknown as RagCollection[]
  return rows
}
