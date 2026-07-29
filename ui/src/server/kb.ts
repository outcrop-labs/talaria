// The knowledgebase — an Outline-style markdown drive. Spaces group docs; docs
// nest. Every save snapshots a version (reusing internal_versions). Marking a
// doc "official" indexes it into the org-kb RAG collection so agents ground on
// it; un-officializing / deleting removes it.
import { randomBytes } from 'node:crypto'
import { db } from './db/pg'
import { snapshot } from './internal-history'
import { syncKbDoc, unindexKbDoc } from './retrieval/sources'
import { canRead, listEditors, type EditorGrant, type Guarded } from './kb-perms'

/** Resolve a doc's effective audience: when it inherits, visibility / edit
 *  policy / grants come from its folder, but ownership always stays with the
 *  doc's own creator (so the author never loses edit rights). */
export async function effectiveDocPerms(doc: KbDoc): Promise<{ perms: Guarded; grants: EditorGrant[] }> {
  if (!doc.permsInherited) {
    return {
      perms: { visibility: doc.visibility, editPolicy: doc.editPolicy, ownerUserId: doc.ownerUserId, createdBy: doc.createdBy },
      grants: await listEditors('doc', doc.id),
    }
  }
  const space = await getSpace(doc.spaceId)
  return {
    perms: {
      visibility: space?.visibility ?? doc.visibility,
      editPolicy: space?.editPolicy ?? doc.editPolicy,
      ownerUserId: doc.ownerUserId,
      createdBy: doc.createdBy,
    },
    grants: space ? await listEditors('space', doc.spaceId) : [],
  }
}

export interface KbSpace {
  id: string
  name: string
  description: string | null
  icon: string | null
  body: string
  visibility: 'private' | 'org' | 'public'
  publicSlug: string | null
  editPolicy: 'owner' | 'org' | 'restricted'
  ownerUserId: string | null
  createdBy: string | null
  createdAt: string
  /** The space's generated OKF digest doc (Librarian-maintained). */
  okfDocId?: string | null
}

export interface KbDocMeta {
  id: string
  spaceId: string
  parentId: string | null
  title: string
  icon: string | null
  kind: 'human' | 'agent'
  official: boolean
  visibility: 'private' | 'org' | 'public'
  publicSlug: string | null
  editPolicy: 'owner' | 'org' | 'restricted'
  permsInherited: boolean
  ownerUserId: string | null
  sort: number
  /** RAG routing: 'auto' (space binding / org rules) | 'none' | a brain id. */
  ragRouting: string
  createdBy: string | null
  updatedBy: string | null
  updatedAt: string
}

export interface KbDoc extends KbDocMeta {
  body: string
  /** Hidden agent-facing OKF concept (frontmatter + summary), Librarian-written. */
  okf?: string | null
}

// ── Spaces ────────────────────────────────────────────────────────────────────
const SPACE_COLS = `id, name, description, icon, body, visibility, public_slug as "publicSlug", okf_doc_id as "okfDocId",
  edit_policy as "editPolicy", owner_user_id as "ownerUserId", created_by as "createdBy", created_at as "createdAt"`

export async function listSpaces(): Promise<KbSpace[]> {
  const sql = await db()
  return (await sql.unsafe(`select ${SPACE_COLS} from kb_spaces order by name asc`)) as unknown as KbSpace[]
}

export async function getSpace(id: string): Promise<KbSpace | null> {
  const sql = await db()
  const rows = (await sql.unsafe(`select ${SPACE_COLS} from kb_spaces where id = $1`, [id])) as unknown as KbSpace[]
  return rows[0] ?? null
}

export async function createSpace(input: { name: string; description?: string; icon?: string; createdBy: string; ownerUserId?: string | null }): Promise<KbSpace> {
  const sql = await db()
  const rows = (await sql`
    insert into kb_spaces (name, description, icon, created_by, owner_user_id)
    values (${input.name}, ${input.description ?? null}, ${input.icon ?? null}, ${input.createdBy}, ${input.ownerUserId ?? null})
    returning ${sql.unsafe(SPACE_COLS)}
  `) as unknown as KbSpace[]
  return rows[0]!
}

export async function updateSpace(
  id: string,
  patch: { name?: string; description?: string | null; icon?: string | null; body?: string; visibility?: 'private' | 'org' | 'public'; editPolicy?: 'owner' | 'org' | 'restricted' },
  actor?: string,
): Promise<KbSpace | null> {
  const sql = await db()
  if (patch.name !== undefined) await sql`update kb_spaces set name = ${patch.name} where id = ${id}`
  if (patch.description !== undefined) await sql`update kb_spaces set description = ${patch.description} where id = ${id}`
  if (patch.icon !== undefined) await sql`update kb_spaces set icon = ${patch.icon} where id = ${id}`
  if (patch.body !== undefined) await sql`update kb_spaces set body = ${patch.body} where id = ${id}`
  if (patch.editPolicy !== undefined) await sql`update kb_spaces set edit_policy = ${patch.editPolicy} where id = ${id}`
  if (patch.visibility !== undefined) {
    await sql`update kb_spaces set visibility = ${patch.visibility} where id = ${id}`
    if (patch.visibility === 'public') {
      await sql`update kb_spaces set public_slug = ${randomBytes(8).toString('hex')} where id = ${id} and public_slug is null`
    }
  }
  const next = await getSpace(id)
  // Version the overview like a doc (name + body snapshot) on content change.
  if (next && (patch.body !== undefined || patch.name !== undefined)) {
    await snapshot('kb-space', id, `# ${next.name}\n\n${next.body}`, actor ?? null).catch(() => {})
  }
  return next
}

export async function getPublicSpace(slug: string): Promise<KbSpace | null> {
  const sql = await db()
  const rows = (await sql.unsafe(`select ${SPACE_COLS} from kb_spaces where public_slug = $1 and visibility = 'public'`, [slug])) as unknown as KbSpace[]
  return rows[0] ?? null
}

export async function deleteSpace(id: string): Promise<void> {
  const sql = await db()
  // Unindex every doc in the space (org brain + any personal collections).
  const docs = (await sql`select id, owner_user_id as "ownerUserId" from kb_docs where space_id = ${id}`) as unknown as Array<{ id: string; ownerUserId: string | null }>
  for (const d of docs) await unindexKbDoc(d.id, d.ownerUserId).catch(() => {})
  await sql`delete from kb_spaces where id = ${id}`
}

// ── Docs ──────────────────────────────────────────────────────────────────────
const DOC_META = `select id, space_id as "spaceId", parent_id as "parentId", title, icon, kind, official,
  visibility, public_slug as "publicSlug", edit_policy as "editPolicy", perms_inherited as "permsInherited",
  owner_user_id as "ownerUserId", sort, rag_routing as "ragRouting",
  created_by as "createdBy", updated_by as "updatedBy", updated_at as "updatedAt" from kb_docs`

export async function listDocs(spaceId: string): Promise<KbDocMeta[]> {
  const sql = await db()
  return (await sql.unsafe(`${DOC_META} where space_id = $1 order by sort asc, title asc`, [spaceId])) as unknown as KbDocMeta[]
}

export async function getDoc(id: string): Promise<KbDoc | null> {
  const sql = await db()
  const rows = (await sql.unsafe(
    `select id, space_id as "spaceId", parent_id as "parentId", title, icon, body, okf, kind, official,
            visibility, public_slug as "publicSlug", edit_policy as "editPolicy", perms_inherited as "permsInherited",
            sort, owner_user_id as "ownerUserId", rag_routing as "ragRouting",
            created_by as "createdBy", updated_by as "updatedBy",
            updated_at as "updatedAt" from kb_docs where id = $1`,
    [id],
  )) as unknown as KbDoc[]
  return rows[0] ?? null
}

export async function getPublicDoc(slug: string): Promise<KbDoc | null> {
  const sql = await db()
  const rows = (await sql`
    select id, space_id as "spaceId", parent_id as "parentId", title, icon, body, okf, kind, official,
           visibility, public_slug as "publicSlug", edit_policy as "editPolicy", perms_inherited as "permsInherited",
           sort, owner_user_id as "ownerUserId",
           created_by as "createdBy", updated_by as "updatedBy", updated_at as "updatedAt"
    from kb_docs where public_slug = ${slug} and visibility = 'public'
  `) as unknown as KbDoc[]
  return rows[0] ?? null
}

// A starter body for OKF-structured agent docs (machine-consumable).
const OKF_TEMPLATE = `---
title:
summary:
tags: []
source: talaria-kb
---

## Overview

## Facts

## Procedures
`

export async function createDoc(input: {
  spaceId: string
  parentId?: string | null
  title?: string
  kind?: 'human' | 'agent'
  createdBy: string
  ownerUserId?: string | null
}): Promise<KbDoc> {
  const sql = await db()
  const body = input.kind === 'agent' ? OKF_TEMPLATE : ''
  const rows = (await sql`
    insert into kb_docs (space_id, parent_id, title, body, kind, created_by, updated_by, owner_user_id)
    values (${input.spaceId}, ${input.parentId ?? null}, ${input.title ?? 'Untitled'}, ${body}, ${input.kind ?? 'human'}, ${input.createdBy}, ${input.createdBy}, ${input.ownerUserId ?? null})
    returning id
  `) as unknown as Array<{ id: string }>
  return (await getDoc(rows[0]!.id))!
}

export async function saveDoc(
  id: string,
  patch: { title?: string; body?: string; icon?: string | null; visibility?: 'private' | 'org' | 'public'; editPolicy?: 'owner' | 'org' | 'restricted'; permsInherited?: boolean; parentId?: string | null },
  actor: string,
): Promise<KbDoc | null> {
  const sql = await db()
  const prev = await getDoc(id)
  if (!prev) return null

  if (patch.title !== undefined) await sql`update kb_docs set title = ${patch.title}, updated_by = ${actor}, updated_at = now() where id = ${id}`
  if (patch.body !== undefined) await sql`update kb_docs set body = ${patch.body}, updated_by = ${actor}, updated_at = now() where id = ${id}`
  if (patch.icon !== undefined) await sql`update kb_docs set icon = ${patch.icon}, updated_at = now() where id = ${id}`
  if (patch.permsInherited !== undefined) await sql`update kb_docs set perms_inherited = ${patch.permsInherited}, updated_at = now() where id = ${id}`
  if (patch.editPolicy !== undefined) await sql`update kb_docs set edit_policy = ${patch.editPolicy}, updated_at = now() where id = ${id}`
  if (patch.visibility) {
    await sql`update kb_docs set visibility = ${patch.visibility}, updated_at = now() where id = ${id}`
    // Public docs get a stable slug on first publish.
    if (patch.visibility === 'public' && !prev.publicSlug) {
      await sql`update kb_docs set public_slug = ${randomBytes(8).toString('hex')} where id = ${id} and public_slug is null`
    }
  }
  if (patch.parentId !== undefined) await sql`update kb_docs set parent_id = ${patch.parentId}, updated_at = now() where id = ${id}`

  const next = await getDoc(id)
  if (next) {
    // Snapshot on content change; re-route RAG on any content/visibility change.
    if (patch.body !== undefined || patch.title !== undefined) {
      await snapshot('kb-doc', id, `# ${next.title}\n\n${next.body}`, actor).catch(() => {})
    }
    if (patch.body !== undefined || patch.title !== undefined || patch.visibility !== undefined || patch.permsInherited !== undefined) {
      await syncDocEffective(next).catch(() => {})
    }
  }
  return next
}

/** Sync a doc into RAG using its EFFECTIVE visibility (inherited from the folder
 *  when applicable) — so an inherited-private doc never lands in the org brain. */
async function syncDocEffective(doc: KbDoc): Promise<void> {
  const { perms } = await effectiveDocPerms(doc)
  await syncKbDoc({ ...doc, visibility: perms.visibility })
}

export async function deleteDoc(id: string): Promise<void> {
  const sql = await db()
  const doc = await getDoc(id)
  await unindexKbDoc(id, doc?.ownerUserId ?? null).catch(() => {})
  await sql`delete from kb_docs where id = ${id}`
}

/** Set a doc's RAG routing ('auto' | 'none' | a custom brain id) and re-place
 *  it immediately. Owner-only at the route layer — routing changes who can
 *  retrieve the doc's content. */
export async function setDocRouting(id: string, routing: string, actor: string): Promise<KbDoc | null> {
  const sql = await db()
  if (routing !== 'auto' && routing !== 'none') {
    const ok = await sql`select 1 from rag_collections where id = ${routing} and kind = 'custom'`
    if (!ok.length) throw new Error('unknown brain')
  }
  await sql`update kb_docs set rag_routing = ${routing}, updated_by = ${actor}, updated_at = now() where id = ${id}`
  const doc = await getDoc(id)
  if (!doc) return null
  await syncDocEffective(doc).catch(() => {})
  return doc
}

/** Mark a doc official (→ org brain) or not. Re-routes its RAG placement. */
export async function setOfficial(id: string, official: boolean, actor: string): Promise<KbDoc | null> {
  const sql = await db()
  await sql`update kb_docs set official = ${official}, updated_by = ${actor}, updated_at = now() where id = ${id}`
  const doc = await getDoc(id)
  if (!doc) return null
  await syncDocEffective(doc).catch(() => {})
  return doc
}

/** Index a user's existing private docs into their (freshly created) personal
 *  RAG collection — called when they spin up their assistant. */
export async function syncUserPrivateDocs(userId: string): Promise<void> {
  const sql = await db()
  const ids = (await sql`select id from kb_docs where owner_user_id = ${userId} and visibility = 'private'`) as unknown as Array<{ id: string }>
  for (const { id } of ids) {
    const doc = await getDoc(id)
    if (doc) await syncKbDoc(doc).catch(() => {})
  }
}

/** Reparent + reorder a doc within its space (the sidebar tree drag target).
 *  A doc can't be nested under itself or a descendant. */
export async function moveDoc(id: string, parentId: string | null, sort: number): Promise<KbDocMeta | null> {
  const sql = await db()
  if (parentId) {
    if (parentId === id) return null
    // Walk up from the proposed parent; if we reach `id`, this is a cycle.
    let cur: string | null = parentId
    for (let i = 0; i < 100 && cur; i++) {
      if (cur === id) return null
      const rows = (await sql`select parent_id as "parentId" from kb_docs where id = ${cur}`) as unknown as Array<{ parentId: string | null }>
      cur = rows[0]?.parentId ?? null
    }
  }
  await sql`update kb_docs set parent_id = ${parentId}, sort = ${sort}, updated_at = now() where id = ${id}`
  const rows = (await sql.unsafe(`${DOC_META} where id = $1`, [id])) as unknown as KbDocMeta[]
  return rows[0] ?? null
}

export interface KbSearchHit {
  id: string
  spaceId: string
  spaceName: string
  title: string
  icon: string | null
  snippet: string
  visibility: 'private' | 'org' | 'public'
  /** Spaces are documents too (their overview) — hits open the space itself. */
  kind: 'doc' | 'space'
}

/** Full-text search across everything the caller may read: docs AND the
 *  top-level space overviews (a space is itself a document). */
/** Full-text search honoring the EFFECTIVE permission model: a doc inside a
 *  private space is private even when its own row says 'org' (inheritance),
 *  and explicit kb_editors grants admit their grantees. Over-fetch, then
 *  filter with the same canRead the read routes use. */
export async function searchDocs(query: string, viewer: { userId: string; who: string | null }): Promise<KbSearchHit[]> {
  const sql = await db()
  const q = query.trim()
  if (!q) return []
  const hits = (await sql`
    select * from (
      select d.id, d.space_id as "spaceId", s.name as "spaceName", d.title, d.icon, d.visibility, 'doc' as kind,
             ts_headline('english', coalesce(d.body,''), plainto_tsquery('english', ${q}),
               'MaxWords=24, MinWords=8, ShortWord=3, MaxFragments=1') as snippet,
             ts_rank(
               to_tsvector('english', coalesce(d.title,'') || ' ' || coalesce(d.body,'')),
               plainto_tsquery('english', ${q})) as rank
      from kb_docs d
      join kb_spaces s on s.id = d.space_id
      where to_tsvector('english', coalesce(d.title,'') || ' ' || coalesce(d.body,''))
            @@ plainto_tsquery('english', ${q})
      union all
      select s.id, s.id as "spaceId", s.name as "spaceName", s.name as title, s.icon, s.visibility, 'space' as kind,
             ts_headline('english', coalesce(s.body,''), plainto_tsquery('english', ${q}),
               'MaxWords=24, MinWords=8, ShortWord=3, MaxFragments=1') as snippet,
             ts_rank(
               to_tsvector('english', coalesce(s.name,'') || ' ' || coalesce(s.body,'')),
               plainto_tsquery('english', ${q})) as rank
      from kb_spaces s
      where to_tsvector('english', coalesce(s.name,'') || ' ' || coalesce(s.body,''))
            @@ plainto_tsquery('english', ${q})
    ) hits
    order by rank desc
    limit 60
  `) as unknown as KbSearchHit[]

  const out: KbSearchHit[] = []
  for (const h of hits) {
    if (out.length >= 20) break
    try {
      if (h.kind === 'space') {
        const sp = await getSpace(h.id)
        if (sp && canRead(sp, viewer.userId, viewer.who, await listEditors('space', sp.id))) out.push(h)
      } else {
        const d = await getDoc(h.id)
        if (!d) continue
        const { perms, grants } = await effectiveDocPerms(d)
        if (canRead(perms, viewer.userId, viewer.who, grants)) out.push(h)
      }
    } catch {
      /* fail closed: an unreadable hit is dropped */
    }
  }
  return out
}

export interface KbBacklink {
  id: string
  title: string
  icon: string | null
  spaceId: string
}

/** Docs that link to this one (editor links point at /knowledge/<id>). */
export async function getBacklinks(docId: string): Promise<KbBacklink[]> {
  const sql = await db()
  return (await sql`
    select id, title, icon, space_id as "spaceId" from kb_docs
    where id <> ${docId} and body like ${'%/knowledge/' + docId + '%'}
    order by updated_at desc limit 50
  `) as unknown as KbBacklink[]
}
