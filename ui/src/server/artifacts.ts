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
  folderId: string | null
  ownerUserId: string | null
  /** RAG routing: 'auto' (plan/research/official flows) | 'none' | a brain id. */
  ragRouting: string
  googleFileId: string | null
  googleFileUrl: string | null
  createdBy: string | null
  updatedBy: string | null
  createdAt: string
  updatedAt: string
}

const COLS = `id, kind, title, icon, body, content_type as "contentType", storage_ref as "storageRef",
  visibility, edit_policy as "editPolicy", public_slug as "publicSlug", official, kb_doc_id as "kbDocId",
  folder_id as "folderId", owner_user_id as "ownerUserId", rag_routing as "ragRouting", google_file_id as "googleFileId", google_file_url as "googleFileUrl",
  created_by as "createdBy", updated_by as "updatedBy",
  created_at as "createdAt", updated_at as "updatedAt"`
// Table-qualified for joins (artifact_links also has created_by / created_at).
const COLS_A = `a.id, a.kind, a.title, a.icon, a.body, a.content_type as "contentType", a.storage_ref as "storageRef",
  a.visibility, a.edit_policy as "editPolicy", a.public_slug as "publicSlug", a.official, a.kb_doc_id as "kbDocId",
  a.folder_id as "folderId", a.owner_user_id as "ownerUserId", a.rag_routing as "ragRouting", a.google_file_id as "googleFileId", a.google_file_url as "googleFileUrl",
  a.created_by as "createdBy", a.updated_by as "updatedBy",
  a.created_at as "createdAt", a.updated_at as "updatedAt"`

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

export async function createArtifact(input: {
  kind?: ArtifactKind
  title?: string
  createdBy: string
  ownerUserId: string | null
  folderId?: string | null
}): Promise<Artifact> {
  const sql = await db()
  const rows = (await sql`
    insert into artifacts (kind, title, created_by, updated_by, owner_user_id, folder_id)
    values (${input.kind ?? 'doc'}, ${input.title ?? 'Untitled'}, ${input.createdBy}, ${input.createdBy}, ${input.ownerUserId}, ${input.folderId ?? null})
    returning ${sql.unsafe(COLS)}
  `) as unknown as Artifact[]
  return rows[0]!
}

/** Find-or-create a folder by name under a parent (case-insensitive). */
async function findOrCreateFolder(name: string, parentId: string | null, createdBy: string): Promise<string> {
  const sql = await db()
  const rows = (await sql`
    select id from artifact_folders
    where lower(name) = ${name.toLowerCase()} and parent_id is not distinct from ${parentId}
    limit 1
  `) as unknown as Array<{ id: string }>
  if (rows[0]) return rows[0].id
  return (await createFolder({ name, parentId, createdBy })).id
}

/** Find-or-create a folder by NAME at the root — the "file this under X"
 *  spelling agents use. Never throws (null = root). */
export async function namedRootFolder(name: string, createdBy: string): Promise<string | null> {
  try {
    return await findOrCreateFolder(name, null, createdBy)
  } catch {
    return null
  }
}

/** The single root every agent cabinet hangs under. One folder per agent at the
 *  ROOT buried the user's own work under a wall of agent names the moment the
 *  fleet grew; the Files browser now opens on your folders, with the whole
 *  fleet's output one click away. */
export const AGENTS_ROOT = 'Agents'

/** The agent's filing cabinet: "Agents/<Agent label>/<Category>", created on
 *  demand. Auto-created artifacts (plan docs, research reports, agent
 *  documents, media saves, chat summaries, brief mirrors) file here instead of
 *  piling up at the root. Never throws — filing must not be able to kill the
 *  flow that creates the artifact; a null just means "root". */
export async function agentCategoryFolder(agentLabel: string, category: string, createdBy: string): Promise<string | null> {
  try {
    const root = await findOrCreateFolder(AGENTS_ROOT, null, createdBy)
    const top = await findOrCreateFolder(agentLabel, root, createdBy)
    return await findOrCreateFolder(category, top, createdBy)
  } catch {
    return null
  }
}

/** The category folders `agentCategoryFolder` writes. Exported because the
 *  migration that reparented pre-existing cabinets matches on them, and the
 *  browser marks the Agents root as fleet-owned rather than yours. */
export const AGENT_CATEGORIES = ['Documents', 'Media', 'Chat summaries', 'Plans', 'Research', 'Briefs'] as const

export async function saveArtifact(
  id: string,
  patch: { title?: string; body?: string; icon?: string | null; storageRef?: string | null; contentType?: string | null; folderId?: string | null; visibility?: Visibility; editPolicy?: EditPolicy },
  actor: string,
): Promise<Artifact | null> {
  const sql = await db()
  const prev = await getArtifact(id)
  if (!prev) return null
  if (patch.folderId !== undefined) await sql`update artifacts set folder_id = ${patch.folderId}, updated_at = now() where id = ${id}`
  if (patch.title !== undefined) await sql`update artifacts set title = ${patch.title}, updated_by = ${actor}, updated_at = now() where id = ${id}`
  if (patch.body !== undefined) await sql`update artifacts set body = ${patch.body}, updated_by = ${actor}, updated_at = now() where id = ${id}`
  if (patch.icon !== undefined) await sql`update artifacts set icon = ${patch.icon}, updated_at = now() where id = ${id}`
  if (patch.storageRef !== undefined) await sql`update artifacts set storage_ref = ${patch.storageRef}, updated_by = ${actor}, updated_at = now() where id = ${id}`
  if (patch.contentType !== undefined) await sql`update artifacts set content_type = ${patch.contentType}, updated_at = now() where id = ${id}`
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

// ── Folders (organize artifacts into a nestable tree) ───────────────────────
export interface ArtifactFolder {
  id: string
  name: string
  icon: string | null
  parentId: string | null
  visibility: Visibility
  editPolicy: EditPolicy
  ownerUserId: string | null
  createdBy: string | null
  createdAt: string
}
const FOLDER_COLS = `id, name, icon, parent_id as "parentId", visibility, edit_policy as "editPolicy", owner_user_id as "ownerUserId", created_by as "createdBy", created_at as "createdAt"`

/** A folder in the shape the permission checks want. Folders carry the same
 *  three access columns as docs, spaces and artifacts, so the same functions
 *  answer "can this person read it / re-share it" for all four. */
export function guardedFolder(f: ArtifactFolder): Guarded {
  return { ownerUserId: f.ownerUserId, createdBy: f.createdBy, visibility: f.visibility, editPolicy: f.editPolicy }
}

export async function listFolders(): Promise<ArtifactFolder[]> {
  const sql = await db()
  return (await sql.unsafe(`select ${FOLDER_COLS} from artifact_folders order by name asc`)) as unknown as ArtifactFolder[]
}

export async function getFolder(id: string): Promise<ArtifactFolder | null> {
  const sql = await db()
  const rows = (await sql.unsafe(`select ${FOLDER_COLS} from artifact_folders where id = $1`, [id])) as unknown as ArtifactFolder[]
  return rows[0] ?? null
}

/** `ownerUserId` is what separates a person's folder from the workspace's.
 *  A human making a folder owns it; the find-or-create path agents use passes
 *  nothing, so agent cabinets stay ownerless and org-visible — the workspace's,
 *  which is exactly what they are. */
export async function createFolder(input: {
  name: string
  parentId?: string | null
  createdBy: string
  ownerUserId?: string | null
  visibility?: Visibility
}): Promise<ArtifactFolder> {
  const sql = await db()
  const rows = (await sql`
    insert into artifact_folders (name, parent_id, created_by, owner_user_id, visibility)
    values (${input.name}, ${input.parentId ?? null}, ${input.createdBy}, ${input.ownerUserId ?? null}, ${input.visibility ?? 'org'})
    returning ${sql.unsafe(FOLDER_COLS)}
  `) as unknown as ArtifactFolder[]
  return rows[0]!
}

/** Rename / set icon / reparent / re-share a folder. Rejects parent cycles. */
export async function updateFolder(
  id: string,
  patch: { name?: string; icon?: string | null; parentId?: string | null; visibility?: Visibility; editPolicy?: EditPolicy },
): Promise<ArtifactFolder | null> {
  const sql = await db()
  if (patch.parentId !== undefined && patch.parentId) {
    if (patch.parentId === id) return null
    let cur: string | null = patch.parentId
    for (let i = 0; i < 100 && cur; i++) {
      if (cur === id) return null // cycle
      const rows = (await sql`select parent_id as "parentId" from artifact_folders where id = ${cur}`) as unknown as Array<{ parentId: string | null }>
      cur = rows[0]?.parentId ?? null
    }
  }
  if (patch.name !== undefined) await sql`update artifact_folders set name = ${patch.name} where id = ${id}`
  if (patch.icon !== undefined) await sql`update artifact_folders set icon = ${patch.icon} where id = ${id}`
  if (patch.parentId !== undefined) await sql`update artifact_folders set parent_id = ${patch.parentId} where id = ${id}`
  if (patch.visibility !== undefined) await sql`update artifact_folders set visibility = ${patch.visibility} where id = ${id}`
  if (patch.editPolicy !== undefined) await sql`update artifact_folders set edit_policy = ${patch.editPolicy} where id = ${id}`
  const rows = (await sql.unsafe(`select ${FOLDER_COLS} from artifact_folders where id = $1`, [id])) as unknown as ArtifactFolder[]
  return rows[0] ?? null
}

/** Delete a folder — its artifacts and child folders fall back to the root
 *  (on delete set null), so nothing is lost. */
export async function deleteFolder(id: string): Promise<void> {
  const sql = await db()
  await sql`delete from artifact_folders where id = ${id}`
}

// ── Attachments (attach an artifact to anything) ────────────────────────────
export interface ArtifactTarget {
  targetType: string
  targetId: string
}

export async function attachArtifact(artifactId: string, target: ArtifactTarget, actor: string): Promise<void> {
  const sql = await db()
  await sql`
    insert into artifact_links (artifact_id, target_type, target_id, created_by)
    values (${artifactId}, ${target.targetType}, ${target.targetId}, ${actor})
    on conflict do nothing
  `
}

export async function detachArtifact(artifactId: string, target: ArtifactTarget): Promise<void> {
  const sql = await db()
  await sql`delete from artifact_links where artifact_id = ${artifactId} and target_type = ${target.targetType} and target_id = ${target.targetId}`
}

/** Artifacts attached to a given target (e.g. a KB doc). */
export async function artifactsForTarget(targetType: string, targetId: string): Promise<Artifact[]> {
  const sql = await db()
  return (await sql.unsafe(
    `select ${COLS_A} from artifacts a
     join artifact_links l on l.artifact_id = a.id
     where l.target_type = $1 and l.target_id = $2 order by l.created_at desc`,
    [targetType, targetId],
  )) as unknown as Artifact[]
}

/** Set an artifact's RAG routing ('auto' | 'none' | a custom brain id). The
 *  caller applies the re-placement (retrieval/artifact-routing) — split to
 *  keep this module free of retrieval imports. */
export async function setArtifactRouting(id: string, routing: string, actor: string): Promise<Artifact | null> {
  const sql = await db()
  if (routing !== 'auto' && routing !== 'none') {
    const ok = await sql`select 1 from rag_collections where id = ${routing} and kind = 'custom'`
    if (!ok.length) throw new Error('unknown brain')
  }
  await sql`update artifacts set rag_routing = ${routing}, updated_by = ${actor}, updated_at = now() where id = ${id}`
  return getArtifact(id)
}

/** The things an artifact is attached to (for its "Attached to" list). */
export async function targetsForArtifact(artifactId: string): Promise<ArtifactTarget[]> {
  const sql = await db()
  return (await sql`
    select target_type as "targetType", target_id as "targetId" from artifact_links where artifact_id = ${artifactId}
  `) as unknown as ArtifactTarget[]
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
export function artifactToMarkdown(a: Artifact): string {
  if (a.kind === 'file') return '' // no text body
  if (a.kind === 'sheet') return sheetToMarkdownTable(a.body)
  return a.body // doc + microsite are already text/markdown
}

/** A sheet's JSON grid (`string[][]`, row 0 = header) → a GFM markdown table. */
function sheetToMarkdownTable(body: string): string {
  let grid: string[][]
  try {
    const g = JSON.parse(body)
    grid = Array.isArray(g) ? g.map((r: unknown[]) => (Array.isArray(r) ? r.map((c) => String(c ?? '')) : [])) : []
  } catch {
    return body
  }
  if (!grid.length) return ''
  const esc = (s: string) => s.replace(/\|/g, '\\|').replace(/\n/g, ' ')
  const [head = [], ...rows] = grid
  const cols = head.length
  const line = (cells: string[]) => `| ${Array.from({ length: cols }, (_, i) => esc(cells[i] ?? '')).join(' | ')} |`
  return [line(head), `| ${Array.from({ length: cols }, () => '---').join(' | ')} |`, ...rows.map(line)].join('\n')
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

/** Record where an artifact was mirrored into Google Drive (last export wins). */
export async function recordGoogleExport(id: string, fileId: string, url: string): Promise<void> {
  const sql = await db()
  await sql`update artifacts set google_file_id = ${fileId}, google_file_url = ${url}, updated_at = now() where id = ${id}`
}

/** Keep the KB mirror fresh when an already-official artifact's content changes. */
export async function remirrorIfOfficial(id: string, actor: string): Promise<void> {
  const a = await getArtifact(id)
  if (a?.official && a.kbDocId) await saveDoc(a.kbDocId, { title: a.title, body: artifactToMarkdown(a) }, actor).catch(() => {})
}
