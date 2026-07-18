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
import { activeTarget, getStorageConfig, internalTarget, replicaTarget, s3Get, s3Put, targetReady, type BucketTarget } from './storage'

export const UPLOADS_DIR = () => process.env.TALARIA_UPLOADS_DIR ?? resolve(process.cwd(), '.uploads')

export interface Attachment {
  id: string
  filename: string
  mime: string
  size: number
  /** Set on knowledge/artifact reference chips (see refs.ts); uploads omit it. */
  refType?: 'kb-doc' | 'artifact'
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
  const active = await activeTarget(cfg)
  if (active) {
    const key = `${active.target.prefix}uploads/${id}${safeExt}`
    await s3Put(active.target, key, input.bytes, input.mime)
    path = `${active.internal ? 's3+internal' : 's3'}://${active.target.bucket}/${key}`
  } else {
    const dir = UPLOADS_DIR()
    await mkdir(dir, { recursive: true })
    path = join(dir, `${id}${safeExt}`)
    await writeFile(path, input.bytes)
  }
  // Mirror to the replica off the request path — a replica outage must never
  // fail an upload. The full sync catches anything missed here.
  const replica = replicaTarget(cfg)
  if (replica) {
    void s3Put(replica, `${replica.prefix}uploads/${id}${safeExt}`, input.bytes, input.mime).catch((e) =>
      console.error('[storage] replica write failed:', e instanceof Error ? e.message : e),
    )
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

/** Read a blob from wherever its recorded path says it lives — the scheme
 *  names the home (`s3+internal://` = bundled container via env creds,
 *  `s3://` = the saved external target, else a filesystem path), so every blob
 *  stays readable regardless of the CURRENT mode. If the home fails and a
 *  replica is configured, fall back to the mirror. */
async function readBlob(path: string): Promise<Buffer | null> {
  const cfg = await getStorageConfig()
  const primary = await (async () => {
    const m = /^(s3\+internal|s3):\/\/([^/]+)\/(.+)$/.exec(path)
    if (!m) return readFile(path).catch(() => null)
    const target: BucketTarget = m[1] === 's3+internal' ? internalTarget() : { ...cfg, bucket: m[2]! }
    if (!targetReady(target)) return null
    return s3Get(target, m[3]!).catch(() => null)
  })()
  if (primary) return primary
  const replica = replicaTarget(cfg)
  if (!replica) return null
  return s3Get(replica, `${replica.prefix}uploads/${blobBasename(path)}`).catch(() => null)
}

/** Canonical `<id><ext>` tail — the same for a disk path and a bucket key, so
 *  replica keys are derivable from any row. */
const blobBasename = (path: string) => path.slice(path.lastIndexOf('/') + 1)

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
export async function uploadStats(): Promise<{ local: number; s3: number; internal: number; localBytes: number }> {
  const sql = await db()
  const rows = (await sql`
    select count(*) filter (where path like 's3://%') as "s3",
           count(*) filter (where path like 's3+internal://%') as "internal",
           count(*) filter (where path not like 's3%') as "local",
           coalesce(sum(size) filter (where path not like 's3%'), 0) as "localBytes"
    from uploads
  `) as unknown as Array<{ s3: number; internal: number; local: number; localBytes: number }>
  const r = rows[0]!
  return { local: Number(r.local), s3: Number(r.s3), internal: Number(r.internal), localBytes: Number(r.localBytes) }
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

/** Move every local-disk blob into the active bucket (internal or external).
 *  Runs detached; poll migrateStatus(). Local files are left in place (the
 *  row's path is the source of truth) — uploads-dir cleanup is the operator's
 *  call. */
export async function migrateUploadsToS3(): Promise<MigrateStatus> {
  const prior = await migrateStatus()
  if (prior?.running) return prior
  const cfg = await getStorageConfig()
  const active = await activeTarget(cfg)
  if (!active) throw new Error('object storage is not configured')
  const scheme = active.internal ? 's3+internal' : 's3'
  const sql = await db()
  const rows = (await sql`
    select id, path, mime from uploads where path not like 's3%' order by created_at asc
  `) as unknown as Array<{ id: string; path: string; mime: string }>
  const status: MigrateStatus = { running: true, moved: 0, failed: 0, total: rows.length }
  await setSetting(MIGRATE_KEY, status)
  void (async () => {
    try {
      for (const r of rows) {
        try {
          const bytes = await readFile(r.path)
          const ext = extname(r.path).replace(/[^.a-z0-9]/gi, '').slice(0, 12)
          const key = `${active.target.prefix}uploads/${r.id}${ext}`
          await s3Put(active.target, key, bytes, r.mime)
          await sql`update uploads set path = ${`${scheme}://${active.target.bucket}/${key}`} where id = ${r.id}`
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

const SYNC_KEY = 'storage_sync_status'

export async function syncStatus(): Promise<MigrateStatus | null> {
  return getSetting<MigrateStatus | null>(SYNC_KEY, null)
}

/** Copy EVERY blob (disk, internal, external — wherever each lives) into the
 *  replica bucket, keyed canonically so per-upload mirror writes and this full
 *  sync land on the same objects. Runs detached; poll syncStatus(). */
export async function syncUploadsToReplica(): Promise<MigrateStatus> {
  const prior = await syncStatus()
  if (prior?.running) return prior
  const cfg = await getStorageConfig()
  const replica = replicaTarget(cfg)
  if (!replica) throw new Error('replica is not configured (enable it and fill in every field)')
  const sql = await db()
  const rows = (await sql`select id, path, mime from uploads order by created_at asc`) as unknown as Array<{
    id: string
    path: string
    mime: string
  }>
  const status: MigrateStatus = { running: true, moved: 0, failed: 0, total: rows.length }
  await setSetting(SYNC_KEY, status)
  void (async () => {
    try {
      for (const r of rows) {
        try {
          const bytes = await readBlob(r.path)
          if (!bytes) throw new Error('unreadable')
          await s3Put(replica, `${replica.prefix}uploads/${blobBasename(r.path)}`, bytes, r.mime)
          status.moved++
        } catch {
          status.failed++
        }
        if ((status.moved + status.failed) % 10 === 0) await setSetting(SYNC_KEY, status)
      }
    } catch (e) {
      status.error = e instanceof Error ? e.message : String(e)
    }
    status.running = false
    status.finishedAt = new Date().toISOString()
    await setSetting(SYNC_KEY, status)
  })()
  return status
}

// Mirrors the toolkit's TEXTUAL check: formats whose bytes read as prose.
const TEXT_MIME = /^text\/|^application\/(json|xml|javascript|x-yaml|yaml|csv|toml|sql|markdown)|(\+json|\+xml)$/
const FILE_CLIP = 6_000

/** The prompt block a message's TEXTUAL file uploads contribute — the file
 *  analogue of refBlocks(). Reads bytes on every call (history rebuilds
 *  included), so callers should scope it to recent messages. */
export async function attachmentTextBlocks(attachments: unknown, maxFiles = 3): Promise<string> {
  if (!Array.isArray(attachments)) return ''
  const files = attachments.filter(
    (a): a is Attachment =>
      !!a && typeof a === 'object' && !('refType' in a) && typeof (a as Attachment).mime === 'string' && TEXT_MIME.test((a as Attachment).mime),
  )
  const blocks: string[] = []
  for (const f of files.slice(0, maxFiles)) {
    const up = await getUpload(f.id).catch(() => null)
    if (!up) continue
    const text = up.bytes.toString('utf8')
    const clipped = text.length > FILE_CLIP ? `${text.slice(0, FILE_CLIP)}\n[clipped]` : text
    blocks.push(`\n\n--- Attached file: "${up.filename}" ---\n${clipped}`)
  }
  return blocks.join('')
}

/** As an OpenAI-style image_url data URL — used to give vision models the
 *  actual image regardless of network reachability to Talaria. */
export async function attachmentAsDataUrl(id: string): Promise<string | null> {
  const up = await getUpload(id)
  if (!up || !isImage(up.mime)) return null
  return `data:${up.mime};base64,${up.bytes.toString('base64')}`
}
