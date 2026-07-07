// Artifacts — versioned work products (doc / sheet / microsite / file) with
// their own hosting, sharing, and version history. Sharing reuses the KB
// permission model (kb_editors with item_type='artifact' + visibility / edit
// policy); versioning reuses internal_versions (kind 'artifact'). Flat, so no
// folder inheritance — an artifact carries its own audience.
import { randomBytes } from 'node:crypto'
import { db } from './db/pg'
import { snapshot } from './internal-history'
import type { EditPolicy, Guarded, Visibility } from './kb-perms'

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
  }
  return next
}

export async function deleteArtifact(id: string): Promise<void> {
  const sql = await db()
  await sql`delete from artifacts where id = ${id}`
}
