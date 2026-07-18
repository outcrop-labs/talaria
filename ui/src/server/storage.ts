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

export interface StorageConfig {
  mode: 'local' | 's3'
  endpoint: string // e.g. https://s3.us-west-004.backblazeb2.com
  region: string // blank = derived from endpoint, else us-east-1
  bucket: string
  accessKeyId: string
  secretAccessKey: string // sealed at rest
  pathStyle: boolean // true works everywhere (B2, R2, MinIO); false = virtual-host
  prefix: string // key prefix inside the bucket, e.g. "talaria/"
}

const DEFAULTS: StorageConfig = {
  mode: 'local',
  endpoint: '',
  region: '',
  bucket: '',
  accessKeyId: '',
  secretAccessKey: '',
  pathStyle: true,
  prefix: '',
}

const KEY = 'storage_config'

export async function getStorageConfig(): Promise<StorageConfig> {
  const raw = await getSetting<Partial<StorageConfig>>(KEY, {})
  const cfg = { ...DEFAULTS, ...raw }
  if (cfg.secretAccessKey) {
    try {
      cfg.secretAccessKey = open(cfg.secretAccessKey)
    } catch {
      cfg.secretAccessKey = ''
    }
  }
  return cfg
}

export async function setStorageConfig(cfg: StorageConfig): Promise<void> {
  await setSetting(KEY, { ...cfg, secretAccessKey: cfg.secretAccessKey ? seal(cfg.secretAccessKey) : '' })
}

/** For the admin GET: never returns the secret, only whether one is set. */
export async function publicStorageConfig(): Promise<Omit<StorageConfig, 'secretAccessKey'> & { hasSecret: boolean }> {
  const { secretAccessKey, ...rest } = await getStorageConfig()
  return { ...rest, hasSecret: !!secretAccessKey }
}

export function s3Ready(cfg: StorageConfig): boolean {
  return cfg.mode === 's3' && !!cfg.endpoint && !!cfg.bucket && !!cfg.accessKeyId && !!cfg.secretAccessKey
}

// ── SigV4 ─────────────────────────────────────────────────────────────────────

const sha256 = (data: Uint8Array | string) => createHash('sha256').update(data).digest('hex')
const hmac = (key: Buffer | string, data: string) => createHmac('sha256', key).update(data).digest()

/** Derive the region when the field is blank: B2 and AWS embed it in the host
 *  (s3.us-west-004.backblazeb2.com, s3.eu-central-1.amazonaws.com); R2 wants
 *  the literal "auto". Fall back to us-east-1, which MinIO also accepts. */
export function regionFor(cfg: StorageConfig): string {
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

function objectUrl(cfg: StorageConfig, key: string): URL {
  const base = new URL(cfg.endpoint)
  if (cfg.pathStyle) {
    base.pathname = `${base.pathname.replace(/\/$/, '')}/${cfg.bucket}/${encodeKey(key)}`
  } else {
    base.host = `${cfg.bucket}.${base.host}`
    base.pathname = `/${encodeKey(key)}`
  }
  return base
}

async function s3Fetch(
  cfg: StorageConfig,
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

export async function s3Put(cfg: StorageConfig, key: string, bytes: Uint8Array, mime: string): Promise<void> {
  const res = await s3Fetch(cfg, 'PUT', key, bytes, mime || 'application/octet-stream')
  if (!res.ok) throw new Error(`storage PUT ${res.status}: ${(await res.text()).slice(0, 300)}`)
}

export async function s3Get(cfg: StorageConfig, key: string): Promise<Buffer | null> {
  const res = await s3Fetch(cfg, 'GET', key)
  if (res.status === 404) return null
  if (!res.ok) throw new Error(`storage GET ${res.status}: ${(await res.text()).slice(0, 300)}`)
  return Buffer.from(await res.arrayBuffer())
}

export async function s3Delete(cfg: StorageConfig, key: string): Promise<void> {
  const res = await s3Fetch(cfg, 'DELETE', key)
  if (!res.ok && res.status !== 404) throw new Error(`storage DELETE ${res.status}: ${(await res.text()).slice(0, 300)}`)
}

/** Round-trip probe: PUT a tiny object, GET it back, DELETE it. Returns a
 *  human-readable failure reason rather than throwing, for the admin panel. */
export async function testStorage(cfg: StorageConfig): Promise<{ ok: boolean; detail: string }> {
  if (!s3Ready({ ...cfg, mode: 's3' })) return { ok: false, detail: 'endpoint, bucket, access key, and secret are all required' }
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
