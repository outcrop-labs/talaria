// Artifacts — versioned work products (doc / sheet / microsite / file) with
// their own hosting, sharing, and version history. Sharing reuses the KB
// permission model (kb_editors with item_type='artifact' + visibility / edit
// policy); versioning reuses internal_versions (kind 'artifact'). Flat, so no
// folder inheritance — an artifact carries its own audience.
import { randomBytes } from 'node:crypto'
import { db } from './db/pg'
import { snapshot } from './internal-history'
import type { EditPolicy, Guarded, Visibility } from './kb-perms'
import { createDoc, createSpace, deleteDoc, getDoc, listSpaces, saveDoc, setOfficial as setKbDocOfficial } from './kb'

export type ArtifactKind = 'doc' | 'sheet' | 'microsite' | 'file'

export interface Artifact {
  id: string
  kind: ArtifactKind
  title: string
  icon: string | null
  body: string
  contentType: string | null
  storageRef: string | null
  visibility: Visibility
  editPolicy: EditPolicy
  publicSlug: string | null
  official: boolean
  kbDocId: string | null
  ownerUserId: string | null
  createdBy: string | null
  updatedBy: string | null
  createdAt: string
  updatedAt: string
}

const COLS = `id, kind, title, icon, body, content_type as "contentType", storage_ref as "storageRef",
  visibility, edit_policy as "editPolicy", public_slug as "publicSlug", official, kb_doc_id as "kbDocId",
  owner_user_id as "ownerUserId", created_by as "createdBy", updated_by as "updatedBy",
  created_at as "createdAt", updated_at as "updatedAt"`

/** The Guarded view a permission check needs. */
export function guarded(a: Artifact): Guarded {
  return { ownerUserId: a.ownerUserId, createdBy: a.createdBy, visibility: a.visibility, editPolicy: a.editPolicy }
}

export async function listArtifacts(): Promise<Artifact[]> {
  const sql = await db()
  return (await sql.unsafe(`select ${COLS} from artifacts order by updated_at desc`)) as unknown as Artifact[]
}

export async function getArtifact(id: string): Promise<Artifact | null> {
  const sql = await db()
  const rows = (await sql.unsafe(`select ${COLS} from artifacts where id = $1`, [id])) as unknown as Artifact[]
  return rows[0] ?? null
}

export async function getPublicArtifact(slug: string): Promise<Artifact | null> {
  const sql = await db()
  const rows = (await sql.unsafe(`select ${COLS} from artifacts where public_slug = $1 and visibility = 'public'`, [slug])) as unknown as Artifact[]
  return rows[0] ?? null
}

export async function createArtifact(input: { kind?: ArtifactKind; title?: string; createdBy: string; ownerUserId: string | null }): Promise<Artifact> {
  const sql = await db()
  const rows = (await sql`
    insert into artifacts (kind, title, created_by, updated_by, owner_user_id)
    values (${input.kind ?? 'doc'}, ${input.title ?? 'Untitled'}, ${input.createdBy}, ${input.createdBy}, ${input.ownerUserId})
    returning ${sql.unsafe(COLS)}
  `) as unknown as Artifact[]
  return rows[0]!
}

export async function saveArtifact(
  id: string,
  patch: { title?: string; body?: string; icon?: string | null; visibility?: Visibility; editPolicy?: EditPolicy },
  actor: string,
): Promise<Artifact | null> {
  const sql = await db()
  const prev = await getArtifact(id)
  if (!prev) return null
  if (patch.title !== undefined) await sql`update artifacts set title = ${patch.title}, updated_by = ${actor}, updated_at = now() where id = ${id}`
  if (patch.body !== undefined) await sql`update artifacts set body = ${patch.body}, updated_by = ${actor}, updated_at = now() where id = ${id}`
  if (patch.icon !== undefined) await sql`update artifacts set icon = ${patch.icon}, updated_at = now() where id = ${id}`
  if (patch.editPolicy !== undefined) await sql`update artifacts set edit_policy = ${patch.editPolicy}, updated_at = now() where id = ${id}`
  if (patch.visibility !== undefined) {
    await sql`update artifacts set visibility = ${patch.visibility}, updated_at = now() where id = ${id}`
    if (patch.visibility === 'public' && !prev.publicSlug) {
      await sql`update artifacts set public_slug = ${randomBytes(8).toString('hex')} where id = ${id} and public_slug is null`
    }
  }
  const next = await getArtifact(id)
  if (next && (patch.body !== undefined || patch.title !== undefined)) {
    await snapshot('artifact', id, `# ${next.title}\n\n${next.body}`, actor).catch(() => {})
    await remirrorIfOfficial(id, actor).catch(() => {})
  }
  return next
}

export async function deleteArtifact(id: string): Promise<void> {
  const a = await getArtifact(id)
  if (a?.kbDocId) await deleteDoc(a.kbDocId).catch(() => {})
  const sql = await db()
  await sql`delete from artifacts where id = ${id}`
}

// ── Promotion to the knowledgebase ──────────────────────────────────────────
// "Making an artifact official" mirrors it into the KB (as an official doc in a
// system "Artifacts" folder) so it grounds the org brain. The link is kept in
// artifacts.kb_doc_id; un-officializing removes the mirror.
const ARTIFACTS_SPACE = 'Artifacts'

async function ensureArtifactsSpace(actor: string): Promise<string> {
  const existing = (await listSpaces()).find((s) => s.name === ARTIFACTS_SPACE)
  if (existing) return existing.id
  const space = await createSpace({ name: ARTIFACTS_SPACE, icon: '◆', description: 'Official artifacts, mirrored into the knowledgebase.', createdBy: actor })
  return space.id
}

/** Render an artifact's content as markdown for the KB mirror. */
function artifactToMarkdown(a: Artifact): string {
  // doc + microsite bodies are already text/markdown; sheets render as-is until
  // a proper table serializer lands; files have no text body.
  return a.kind === 'file' ? '' : a.body
}

export async function setArtifactOfficial(id: string, official: boolean, actor: string): Promise<Artifact | null> {
  const sql = await db()
  const a = await getArtifact(id)
  if (!a) return null

  if (official) {
    const spaceId = await ensureArtifactsSpace(actor)
    const md = artifactToMarkdown(a)
    let kbDocId = a.kbDocId
    if (!kbDocId || !(await getDoc(kbDocId))) {
      const doc = await createDoc({ spaceId, title: a.title, kind: 'human', createdBy: actor, ownerUserId: a.ownerUserId })
      kbDocId = doc.id
    }
    await saveDoc(kbDocId, { title: a.title, body: md }, actor)
    await setKbDocOfficial(kbDocId, true, actor) // → org brain
    await sql`update artifacts set official = true, kb_doc_id = ${kbDocId}, updated_at = now() where id = ${id}`
  } else {
    if (a.kbDocId) await deleteDoc(a.kbDocId).catch(() => {})
    await sql`update artifacts set official = false, kb_doc_id = null, updated_at = now() where id = ${id}`
  }
  return getArtifact(id)
}

/** Keep the KB mirror fresh when an already-official artifact's content changes. */
export async function remirrorIfOfficial(id: string, actor: string): Promise<void> {
  const a = await getArtifact(id)
  if (a?.official && a.kbDocId) await saveDoc(a.kbDocId, { title: a.title, body: artifactToMarkdown(a) }, actor).catch(() => {})
}
