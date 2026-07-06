// The knowledgebase — an Outline-style markdown drive. Spaces group docs; docs
// nest. Every save snapshots a version (reusing internal_versions). Marking a
// doc "official" indexes it into the org-kb RAG collection so agents ground on
// it; un-officializing / deleting removes it.
import { randomBytes } from 'node:crypto'
import { db } from './db/pg'
import { snapshot } from './internal-history'
import { indexOrgKb, unindexOrgKb } from './retrieval/sources'

export interface KbSpace {
  id: string
  name: string
  description: string | null
  icon: string | null
  createdBy: string | null
  createdAt: string
}

export interface KbDocMeta {
  id: string
  spaceId: string
  parentId: string | null
  title: string
  kind: 'human' | 'agent'
  official: boolean
  visibility: 'private' | 'org' | 'public'
  publicSlug: string | null
  sort: number
  createdBy: string | null
  updatedBy: string | null
  updatedAt: string
}

export interface KbDoc extends KbDocMeta {
  body: string
}

// ── Spaces ────────────────────────────────────────────────────────────────────
export async function listSpaces(): Promise<KbSpace[]> {
  const sql = await db()
  return (await sql`
    select id, name, description, icon, created_by as "createdBy", created_at as "createdAt"
    from kb_spaces order by name asc
  `) as unknown as KbSpace[]
}

export async function createSpace(input: { name: string; description?: string; icon?: string; createdBy: string }): Promise<KbSpace> {
  const sql = await db()
  const rows = (await sql`
    insert into kb_spaces (name, description, icon, created_by)
    values (${input.name}, ${input.description ?? null}, ${input.icon ?? null}, ${input.createdBy})
    returning id, name, description, icon, created_by as "createdBy", created_at as "createdAt"
  `) as unknown as KbSpace[]
  return rows[0]!
}

export async function deleteSpace(id: string): Promise<void> {
  const sql = await db()
  // Unindex any official docs first.
  const docs = (await sql`select id from kb_docs where space_id = ${id} and official`) as unknown as Array<{ id: string }>
  for (const d of docs) await unindexOrgKb('kb-doc', d.id).catch(() => {})
  await sql`delete from kb_spaces where id = ${id}`
}

// ── Docs ──────────────────────────────────────────────────────────────────────
const DOC_META = `select id, space_id as "spaceId", parent_id as "parentId", title, kind, official,
  visibility, public_slug as "publicSlug", sort, created_by as "createdBy", updated_by as "updatedBy",
  updated_at as "updatedAt" from kb_docs`

export async function listDocs(spaceId: string): Promise<KbDocMeta[]> {
  const sql = await db()
  return (await sql.unsafe(`${DOC_META} where space_id = $1 order by sort asc, title asc`, [spaceId])) as unknown as KbDocMeta[]
}

export async function getDoc(id: string): Promise<KbDoc | null> {
  const sql = await db()
  const rows = (await sql.unsafe(
    `select id, space_id as "spaceId", parent_id as "parentId", title, body, kind, official,
            visibility, public_slug as "publicSlug", sort, created_by as "createdBy", updated_by as "updatedBy",
            updated_at as "updatedAt" from kb_docs where id = $1`,
    [id],
  )) as unknown as KbDoc[]
  return rows[0] ?? null
}

export async function getPublicDoc(slug: string): Promise<KbDoc | null> {
  const sql = await db()
  const rows = (await sql`
    select id, space_id as "spaceId", parent_id as "parentId", title, body, kind, official,
           visibility, public_slug as "publicSlug", sort, created_by as "createdBy", updated_by as "updatedBy",
           updated_at as "updatedAt"
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
}): Promise<KbDoc> {
  const sql = await db()
  const body = input.kind === 'agent' ? OKF_TEMPLATE : ''
  const rows = (await sql`
    insert into kb_docs (space_id, parent_id, title, body, kind, created_by, updated_by)
    values (${input.spaceId}, ${input.parentId ?? null}, ${input.title ?? 'Untitled'}, ${body}, ${input.kind ?? 'human'}, ${input.createdBy}, ${input.createdBy})
    returning id
  `) as unknown as Array<{ id: string }>
  return (await getDoc(rows[0]!.id))!
}

export async function saveDoc(
  id: string,
  patch: { title?: string; body?: string; visibility?: 'private' | 'org' | 'public'; parentId?: string | null },
  actor: string,
): Promise<KbDoc | null> {
  const sql = await db()
  const prev = await getDoc(id)
  if (!prev) return null

  if (patch.title !== undefined) await sql`update kb_docs set title = ${patch.title}, updated_by = ${actor}, updated_at = now() where id = ${id}`
  if (patch.body !== undefined) await sql`update kb_docs set body = ${patch.body}, updated_by = ${actor}, updated_at = now() where id = ${id}`
  if (patch.visibility) {
    await sql`update kb_docs set visibility = ${patch.visibility}, updated_at = now() where id = ${id}`
    // Public docs get a stable slug on first publish.
    if (patch.visibility === 'public' && !prev.publicSlug) {
      await sql`update kb_docs set public_slug = ${randomBytes(8).toString('hex')} where id = ${id} and public_slug is null`
    }
  }
  if (patch.parentId !== undefined) await sql`update kb_docs set parent_id = ${patch.parentId}, updated_at = now() where id = ${id}`

  const next = await getDoc(id)
  // Snapshot the version + keep the org brain fresh if this doc is official.
  if (next && (patch.body !== undefined || patch.title !== undefined)) {
    await snapshot('kb-doc', id, `# ${next.title}\n\n${next.body}`, actor).catch(() => {})
    if (next.official) await reindexOfficial(next).catch(() => {})
  }
  return next
}

export async function deleteDoc(id: string): Promise<void> {
  const sql = await db()
  await unindexOrgKb('kb-doc', id).catch(() => {})
  await sql`delete from kb_docs where id = ${id}`
}

/** Mark a doc official (→ org brain) or not (→ removed from it). */
export async function setOfficial(id: string, official: boolean, actor: string): Promise<KbDoc | null> {
  const sql = await db()
  await sql`update kb_docs set official = ${official}, updated_by = ${actor}, updated_at = now() where id = ${id}`
  const doc = await getDoc(id)
  if (!doc) return null
  if (official) await reindexOfficial(doc).catch(() => {})
  else await unindexOrgKb('kb-doc', id).catch(() => {})
  return doc
}

async function reindexOfficial(doc: KbDoc): Promise<void> {
  await indexOrgKb({
    sourceType: 'kb-doc',
    sourceId: doc.id,
    title: doc.title,
    text: `${doc.title}\n\n${doc.body}`,
    href: `/knowledge/${doc.id}`,
  })
}
