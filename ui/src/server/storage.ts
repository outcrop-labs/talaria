// Object storage for upload blobs: any S3-compatible endpoint (AWS S3,
// Backblaze B2, Cloudflare R2, MinIO, ...) behind a hand-rolled SigV4 client —
// the same no-SDK fetch pattern as the Qdrant/TEI clients. Local disk stays the
// default; S3 is opt-in via Admin → Storage. Config lives in app_settings
// (`storage_config`) with the secret key sealed by secretbox. uploads.path
// records where each blob actually lives (`s3://bucket/key` vs a filesystem
// path), so flipping the mode never strands existing files.
import { createHash, createHmac } from 'node:crypto'
import { getSetting, setSetting } from './audit'
import { seal, open } from './secretbox'

export interface BucketTarget {
  endpoint: string // e.g. https://s3.us-west-004.backblazeb2.com
  region: string // blank = derived from endpoint, else us-east-1
  bucket: string
  accessKeyId: string
  secretAccessKey: string // sealed at rest
  pathStyle: boolean // true works everywhere (B2, R2, MinIO); false = virtual-host
  prefix: string // key prefix inside the bucket, e.g. "talaria/"
}

export interface StorageConfig extends BucketTarget {
  // 'internal' = the bundled talaria-minio container (config from TALARIA_S3_*
  // env, bucket auto-created) — a real bucket with no cloud account.
  mode: 'local' | 'internal' | 's3'
  // Optional mirror: every blob written to the primary is also written to the
  // replica (fire-and-forget), and reads fall back to it. "Sync" backfills it.
  replica: BucketTarget & { enabled: boolean }
}

const EMPTY_TARGET: BucketTarget = {
  endpoint: '',
  region: '',
  bucket: '',
  accessKeyId: '',
  secretAccessKey: '',
  pathStyle: true,
  prefix: '',
}

const DEFAULTS: StorageConfig = {
  mode: 'local',
  ...EMPTY_TARGET,
  replica: { enabled: false, ...EMPTY_TARGET },
}

/** The bundled MinIO container. Defaults match docker/dev-compose.yml; `talaria setup`
 *  writes a random secret into ui/.env and `talaria dev` exports it for both sides. */
export function internalTarget(): BucketTarget {
  return {
    endpoint: process.env.TALARIA_S3_URL ?? `http://127.0.0.1:${process.env.TALARIA_MINIO_PORT ?? '9010'}`,
    region: 'us-east-1',
    bucket: process.env.TALARIA_S3_BUCKET ?? 'talaria',
    accessKeyId: process.env.TALARIA_S3_ACCESS_KEY ?? 'talaria',
    secretAccessKey: process.env.TALARIA_S3_SECRET_KEY ?? 'talaria-dev-secret',
    pathStyle: true,
    prefix: '',
  }
}

const KEY = 'storage_config'

const unseal = (token: string) => {
  try {
    return open(token)
  } catch {
    return ''
  }
}

export async function getStorageConfig(): Promise<StorageConfig> {
  const raw = await getSetting<Partial<StorageConfig>>(KEY, {})
  const cfg = { ...DEFAULTS, ...raw, replica: { ...DEFAULTS.replica, ...raw.replica } }
  if (cfg.secretAccessKey) cfg.secretAccessKey = unseal(cfg.secretAccessKey)
  if (cfg.replica.secretAccessKey) cfg.replica.secretAccessKey = unseal(cfg.replica.secretAccessKey)
  return cfg
}

export async function setStorageConfig(cfg: StorageConfig): Promise<void> {
  await setSetting(KEY, {
    ...cfg,
    secretAccessKey: cfg.secretAccessKey ? seal(cfg.secretAccessKey) : '',
    replica: { ...cfg.replica, secretAccessKey: cfg.replica.secretAccessKey ? seal(cfg.replica.secretAccessKey) : '' },
  })
}

/** For the admin GET: never returns secrets, only whether each is set. */
export async function publicStorageConfig(): Promise<
  Omit<StorageConfig, 'secretAccessKey' | 'replica'> & {
    hasSecret: boolean
    replica: Omit<StorageConfig['replica'], 'secretAccessKey'> & { hasSecret: boolean }
  }
> {
  const { secretAccessKey, replica, ...rest } = await getStorageConfig()
  const { secretAccessKey: rSecret, ...rRest } = replica
  return { ...rest, hasSecret: !!secretAccessKey, replica: { ...rRest, hasSecret: !!rSecret } }
}

export function targetReady(t: BucketTarget): boolean {
  return !!t.endpoint && !!t.bucket && !!t.accessKeyId && !!t.secretAccessKey
}

/** Where writes go: the internal container, the configured bucket, or null for
 *  local disk. Internal ensures its bucket exists (idempotent, throttled). */
export async function activeTarget(cfg: StorageConfig): Promise<{ target: BucketTarget; internal: boolean } | null> {
  if (cfg.mode === 'internal') {
    const target = internalTarget()
    await ensureBucket(target)
    return { target, internal: true }
  }
  if (cfg.mode === 's3' && targetReady(cfg)) return { target: cfg, internal: false }
  return null
}

/** The enabled + fully-configured replica, if any. */
export function replicaTarget(cfg: StorageConfig): BucketTarget | null {
  return cfg.replica.enabled && targetReady(cfg.replica) ? cfg.replica : null
}

// ── SigV4 ─────────────────────────────────────────────────────────────────────

const sha256 = (data: Uint8Array | string) => createHash('sha256').update(data).digest('hex')
const hmac = (key: Buffer | string, data: string) => createHmac('sha256', key).update(data).digest()

/** Derive the region when the field is blank: B2 and AWS embed it in the host
 *  (s3.us-west-004.backblazeb2.com, s3.eu-central-1.amazonaws.com); R2 wants
 *  the literal "auto". Fall back to us-east-1, which MinIO also accepts. */
export function regionFor(cfg: BucketTarget): string {
  if (cfg.region.trim()) return cfg.region.trim()
  const host = safeHost(cfg.endpoint)
  const m = /^s3\.([a-z0-9-]+)\.(?:backblazeb2\.com|amazonaws\.com)$/.exec(host)
  if (m) return m[1]!
  if (/\.r2\.cloudflarestorage\.com$/.test(host)) return 'auto'
  return 'us-east-1'
}

function safeHost(endpoint: string): string {
  try {
    return new URL(endpoint).host
  } catch {
    return ''
  }
}

/** RFC 3986 encode a key, keeping `/` as the segment separator. */
const encodeKey = (key: string) =>
  key
    .split('/')
    .map((s) => encodeURIComponent(s).replace(/[!'()*]/g, (c) => `%${c.charCodeAt(0).toString(16).toUpperCase()}`))
    .join('/')

function objectUrl(cfg: BucketTarget, key: string): URL {
  const base = new URL(cfg.endpoint)
  if (cfg.pathStyle) {
    base.pathname = `${base.pathname.replace(/\/$/, '')}/${cfg.bucket}${key ? `/${encodeKey(key)}` : ''}`
  } else {
    base.host = `${cfg.bucket}.${base.host}`
    base.pathname = key ? `/${encodeKey(key)}` : '/'
  }
  return base
}

async function s3Fetch(
  cfg: BucketTarget,
  method: 'GET' | 'PUT' | 'DELETE' | 'HEAD',
  key: string,
  body?: Uint8Array,
  contentType?: string,
): Promise<Response> {
  const url = objectUrl(cfg, key)
  const now = new Date()
  const amzDate = now.toISOString().replace(/[-:]/g, '').replace(/\.\d{3}/, '') // YYYYMMDDTHHMMSSZ
  const dateStamp = amzDate.slice(0, 8)
  const region = regionFor(cfg)
  const payloadHash = sha256(body ?? '')

  const headers: Record<string, string> = {
    host: url.host,
    'x-amz-content-sha256': payloadHash,
    'x-amz-date': amzDate,
  }
  if (contentType) headers['content-type'] = contentType

  const signedNames = Object.keys(headers).sort()
  const canonicalHeaders = signedNames.map((h) => `${h}:${headers[h]!.trim()}\n`).join('')
  const signedHeaders = signedNames.join(';')
  const canonicalRequest = [method, url.pathname, url.searchParams.toString(), canonicalHeaders, signedHeaders, payloadHash].join('\n')

  const scope = `${dateStamp}/${region}/s3/aws4_request`
  const stringToSign = ['AWS4-HMAC-SHA256', amzDate, scope, sha256(canonicalRequest)].join('\n')
  const kSigning = hmac(hmac(hmac(hmac(`AWS4${cfg.secretAccessKey}`, dateStamp), region), 's3'), 'aws4_request')
  const signature = createHmac('sha256', kSigning).update(stringToSign).digest('hex')

  const { host: _host, ...sendHeaders } = headers // fetch sets Host itself
  return fetch(url, {
    method,
    headers: {
      ...sendHeaders,
      authorization: `AWS4-HMAC-SHA256 Credential=${cfg.accessKeyId}/${scope}, SignedHeaders=${signedHeaders}, Signature=${signature}`,
    },
    body: body as BodyInit | undefined,
  })
}

// ── Object operations ─────────────────────────────────────────────────────────

export async function s3Put(cfg: BucketTarget, key: string, bytes: Uint8Array, mime: string): Promise<void> {
  const res = await s3Fetch(cfg, 'PUT', key, bytes, mime || 'application/octet-stream')
  if (!res.ok) throw new Error(`storage PUT ${res.status}: ${(await res.text()).slice(0, 300)}`)
}

export async function s3Get(cfg: BucketTarget, key: string): Promise<Buffer | null> {
  const res = await s3Fetch(cfg, 'GET', key)
  if (res.status === 404) return null
  if (!res.ok) throw new Error(`storage GET ${res.status}: ${(await res.text()).slice(0, 300)}`)
  return Buffer.from(await res.arrayBuffer())
}

export async function s3Delete(cfg: BucketTarget, key: string): Promise<void> {
  const res = await s3Fetch(cfg, 'DELETE', key)
  if (!res.ok && res.status !== 404) throw new Error(`storage DELETE ${res.status}: ${(await res.text()).slice(0, 300)}`)
}

/** CreateBucket, tolerant of "already exists". Cached per process per target so
 *  the internal mode doesn't re-check on every upload. Survives HMR like
 *  secretbox does (globalThis). */
const g = globalThis as unknown as { __talariaBuckets?: Set<string> }
g.__talariaBuckets ??= new Set()

export async function ensureBucket(cfg: BucketTarget): Promise<void> {
  const id = `${cfg.endpoint}|${cfg.bucket}`
  if (g.__talariaBuckets!.has(id)) return
  const res = await s3Fetch(cfg, 'PUT', '')
  if (!res.ok) {
    const body = await res.text()
    if (!/BucketAlready(OwnedByYou|Exists)/.test(body)) throw new Error(`storage create-bucket ${res.status}: ${body.slice(0, 300)}`)
  }
  g.__talariaBuckets!.add(id)
}

/** Round-trip probe: PUT a tiny object, GET it back, DELETE it. Returns a
 *  human-readable failure reason rather than throwing, for the admin panel. */
export async function testStorage(cfg: BucketTarget): Promise<{ ok: boolean; detail: string }> {
  if (!targetReady(cfg)) return { ok: false, detail: 'endpoint, bucket, access key, and secret are all required' }
  const key = `${cfg.prefix}talaria-storage-probe`
  const payload = new TextEncoder().encode('talaria storage probe')
  try {
    await s3Put(cfg, key, payload, 'text/plain')
    const back = await s3Get(cfg, key)
    if (!back || Buffer.from(payload).compare(back) !== 0) return { ok: false, detail: 'wrote the probe object but read back different bytes' }
    await s3Delete(cfg, key).catch(() => {}) // cleanup failure isn't a config failure
    return { ok: true, detail: `bucket "${cfg.bucket}" is reachable and writable (region ${regionFor(cfg)})` }
  } catch (e) {
    return { ok: false, detail: e instanceof Error ? e.message : String(e) }
  }
}
