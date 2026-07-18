// Attachment storage: metadata in `uploads`, bytes wherever the storage config
// says — local disk (default) or any S3-compatible bucket (Admin → Storage).
// Each row's `path` records where ITS bytes live (`s3://bucket/key` vs a
// filesystem path), so changing the mode never strands existing files.
// Messages reference uploads by id; the served URL is /api/uploads/:id.
import { mkdir, readFile, writeFile } from 'node:fs/promises'
import { join, resolve, extname } from 'node:path'
import { randomUUID } from 'node:crypto'
import { db } from './db/pg'
import { getSetting, setSetting } from './audit'
import { getStorageConfig, s3Ready, s3Put, s3Get } from './storage'

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
  let path: string
  const cfg = await getStorageConfig()
  if (s3Ready(cfg)) {
    const key = `${cfg.prefix}uploads/${id}${safeExt}`
    await s3Put(cfg, key, input.bytes, input.mime)
    path = `s3://${cfg.bucket}/${key}`
  } else {
    const dir = UPLOADS_DIR()
    await mkdir(dir, { recursive: true })
    path = join(dir, `${id}${safeExt}`)
    await writeFile(path, input.bytes)
  }
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
  const bytes = await readBlob(r.path)
  if (!bytes) return null
  return { bytes, mime: r.mime, filename: r.filename }
}

/** Read a blob from wherever its recorded path says it lives. An s3:// path is
 *  read with the CURRENT config's credentials — the bucket in the path is
 *  authoritative for the key, the config for how to reach it. */
async function readBlob(path: string): Promise<Buffer | null> {
  const m = /^s3:\/\/([^/]+)\/(.+)$/.exec(path)
  if (!m) return readFile(path).catch(() => null)
  const cfg = await getStorageConfig()
  if (!cfg.endpoint || !cfg.accessKeyId || !cfg.secretAccessKey) return null
  return s3Get({ ...cfg, bucket: m[1]! }, m[2]!).catch(() => null)
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

/** Where blobs live right now, for the admin panel. */
export async function uploadStats(): Promise<{ local: number; s3: number; localBytes: number }> {
  const sql = await db()
  const rows = (await sql`
    select count(*) filter (where path like 's3://%') as "s3",
           count(*) filter (where path not like 's3://%') as "local",
           coalesce(sum(size) filter (where path not like 's3://%'), 0) as "localBytes"
    from uploads
  `) as unknown as Array<{ s3: number; local: number; localBytes: number }>
  const r = rows[0]!
  return { local: Number(r.local), s3: Number(r.s3), localBytes: Number(r.localBytes) }
}

export interface MigrateStatus {
  running: boolean
  moved: number
  failed: number
  total: number
  error?: string
  finishedAt?: string
}

const MIGRATE_KEY = 'storage_migrate_status'

export async function migrateStatus(): Promise<MigrateStatus | null> {
  return getSetting<MigrateStatus | null>(MIGRATE_KEY, null)
}

/** Move every local-disk blob into the configured bucket. Runs detached; poll
 *  migrateStatus(). Local files are left in place (the row's path is the source
 *  of truth) — cleanup of the uploads dir is the operator's call. */
export async function migrateUploadsToS3(): Promise<MigrateStatus> {
  const prior = await migrateStatus()
  if (prior?.running) return prior
  const cfg = await getStorageConfig()
  if (!s3Ready(cfg)) throw new Error('object storage is not configured')
  const sql = await db()
  const rows = (await sql`
    select id, path, mime from uploads where path not like 's3://%' order by created_at asc
  `) as unknown as Array<{ id: string; path: string; mime: string }>
  const status: MigrateStatus = { running: true, moved: 0, failed: 0, total: rows.length }
  await setSetting(MIGRATE_KEY, status)
  void (async () => {
    try {
      for (const r of rows) {
        try {
          const bytes = await readFile(r.path)
          const ext = extname(r.path).replace(/[^.a-z0-9]/gi, '').slice(0, 12)
          const key = `${cfg.prefix}uploads/${r.id}${ext}`
          await s3Put(cfg, key, bytes, r.mime)
          await sql`update uploads set path = ${`s3://${cfg.bucket}/${key}`} where id = ${r.id}`
          status.moved++
        } catch {
          status.failed++
        }
        if ((status.moved + status.failed) % 10 === 0) await setSetting(MIGRATE_KEY, status)
      }
    } catch (e) {
      status.error = e instanceof Error ? e.message : String(e)
    }
    status.running = false
    status.finishedAt = new Date().toISOString()
    await setSetting(MIGRATE_KEY, status)
  })()
  return status
}

/** As an OpenAI-style image_url data URL — used to give vision models the
 *  actual image regardless of network reachability to Talaria. */
export async function attachmentAsDataUrl(id: string): Promise<string | null> {
  const up = await getUpload(id)
  if (!up || !isImage(up.mime)) return null
  return `data:${up.mime};base64,${up.bytes.toString('base64')}`
}
