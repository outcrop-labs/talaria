// Attachment storage: files land on disk (a persistent uploads dir), metadata
// in `uploads`. Messages reference them by id; the served URL is /api/uploads/:id.
// Kept deliberately simple — a self-hosted app owns its own blob store.
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { join, resolve, extname } from 'node:path'
import { randomUUID } from 'node:crypto'
import { db } from './db/pg'

export const UPLOADS_DIR = () => process.env.TALARIA_UPLOADS_DIR ?? resolve(process.cwd(), '.uploads')

export interface Attachment {
  id: string
  filename: string
  mime: string
  size: number
}

const MAX_BYTES = 25 * 1024 * 1024 // 25 MB
// Store-and-serve is safe for any type, but we only ever RENDER images inline;
// everything else is a download. No executable-content rendering.
const isImage = (mime: string) => /^image\//.test(mime)
export { isImage }

export async function saveUpload(input: {
  filename: string
  mime: string
  bytes: Uint8Array
  userId: string | null
}): Promise<Attachment> {
  if (input.bytes.byteLength > MAX_BYTES) throw new Error('file too large (max 25 MB)')
  const id = randomUUID()
  const safeExt = extname(input.filename).replace(/[^.a-z0-9]/gi, '').slice(0, 12)
  const dir = UPLOADS_DIR()
  await mkdir(dir, { recursive: true })
  const path = join(dir, `${id}${safeExt}`)
  await writeFile(path, input.bytes)
  const sql = await db()
  await sql`
    insert into uploads (id, filename, mime, size, path, uploaded_by)
    values (${id}, ${input.filename.slice(0, 300)}, ${input.mime.slice(0, 120)}, ${input.bytes.byteLength}, ${path}, ${input.userId})
  `
  return { id, filename: input.filename, mime: input.mime, size: input.bytes.byteLength }
}

export async function getUpload(id: string): Promise<{ bytes: Buffer; mime: string; filename: string } | null> {
  const sql = await db()
  const rows = (await sql`select filename, mime, path from uploads where id = ${id}`) as unknown as Array<{
    filename: string
    mime: string
    path: string
  }>
  const r = rows[0]
  if (!r) return null
  const bytes = await readFile(r.path).catch(() => null)
  if (!bytes) return null
  return { bytes, mime: r.mime, filename: r.filename }
}

/** Validate that a set of attachment ids exist (before stamping them on a
 *  message) and return their canonical metadata. */
export async function resolveAttachments(ids: string[]): Promise<Attachment[]> {
  if (!ids.length) return []
  const sql = await db()
  const rows = (await sql`
    select id, filename, mime, size from uploads where id = any(${ids})
  `) as unknown as Attachment[]
  // Preserve caller order.
  const byId = new Map(rows.map((r) => [r.id, r]))
  return ids.map((id) => byId.get(id)).filter((a): a is Attachment => !!a)
}

/** As an OpenAI-style image_url data URL — used to give vision models the
 *  actual image regardless of network reachability to Talaria. */
export async function attachmentAsDataUrl(id: string): Promise<string | null> {
  const up = await getUpload(id)
  if (!up || !isImage(up.mime)) return null
  return `data:${up.mime};base64,${up.bytes.toString('base64')}`
}
