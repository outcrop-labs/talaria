// Shared plumbing for `talaria backup` and `talaria restore` — port of
// scripts/backup-lib.sh. Both commands need the same three things: the app's
// own config, a Postgres client, and the real home of the upload blobs —
// which is local disk, the bundled MinIO container, or an external
// S3-compatible bucket depending on what Admin → Storage is set to
// (ui/src/server/storage.ts).

import { createHash } from 'node:crypto'
import { createReadStream, existsSync, readFileSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import type { Ctx } from '../ctx'
import { envValue, readEnvFile } from '../envfile'
import { STORAGE_PORT } from '../ports'

// Client images, only used when the host has no psql/pg_dump/mc. postgres:16
// matches docker/dev-compose.yml — a dump is refused if the client is older
// than the server, so bump this together with the server image.
export const pgImage = (env: Env): string => env.TALARIA_PG_IMAGE || 'postgres:16-alpine'
// rclone, NOT mc. MinIO's client died with the server: docker.io/minio was
// deleted, quay.io/minio/mc answers 401 to an anonymous pull, and dl.min.io
// returns 410 Gone — the image cannot be fetched from anywhere public, and
// unlike the server we never held a cached copy to mirror. rclone replaces
// it: MIT, multi-arch, still published, and it speaks the same S3 the bucket
// already answers. Only two operations were ever used here and both map
// straight across (see rcloneRun).
export const PINNED_RCLONE_IMAGE = 'rclone/rclone:1.71'

/** The rclone image a backup/restore borrows when the host has no rclone
 *  binary. TALARIA_RCLONE_IMAGE overrides. The MinIO-era TALARIA_MC_IMAGE is
 *  REFUSED rather than honoured: the argv below is rclone's, so an mc image
 *  would fail on the first flag instead of at the pull, which is a far worse
 *  error to read. */
export function rcloneImage(ctx: Ctx): string {
  const override = ctx.env.TALARIA_RCLONE_IMAGE
  if (!override && ctx.env.TALARIA_MC_IMAGE) {
    ctx.log.warn(`TALARIA_MC_IMAGE is set, but mc was replaced by rclone — ignoring it; set TALARIA_RCLONE_IMAGE to override ${PINNED_RCLONE_IMAGE}`)
  }
  return override || PINNED_RCLONE_IMAGE
}

type Env = Record<string, string | undefined>

// ── App config ───────────────────────────────────────────────────────────────

/** The runtime view of the app's env: ui/.env (or $TALARIA_ENV_FILE) with the
 *  shell ALWAYS winning, so an operator can back up a remote instance without
 *  a checkout of its .env. Env wins even when empty — the documented footgun;
 *  an empty DATABASE_URL export is a loud error, not a silent fallback. */
export function liftAppEnv(ctx: Ctx): Env {
  // One layer of quotes tolerated: the bash stripped them for files other
  // tools had written by hand. Env wins even when empty.
  return readEnvFile(ctx, ctx.env.TALARIA_ENV_FILE || 'ui/.env', { quotes: true, envWins: true })
}

/** The connection string minus its credentials — safe to print and to record
 *  in a manifest that sits next to the dump. `${URL##*@}`: after the LAST @. */
export function dbLabel(url: string): string {
  const at = url.lastIndexOf('@')
  return at === -1 ? url : url.slice(at + 1)
}

// ── Postgres client ──────────────────────────────────────────────────────────

async function commandExists(ctx: Ctx, bin: string): Promise<boolean> {
  try {
    await ctx.exec(bin, ['--version'])
    return true
  } catch {
    return false
  }
}

export type PgClient =
  | { kind: 'host'; bin: string; pre: string[] }
  /** Borrowed from a throwaway container on the host network, so a 127.0.0.1
   *  URL resolves the same either way (Linux; on macOS/Windows install the
   *  postgres client instead — Docker's host networking isn't the default). */
  | { kind: 'docker'; pre: string[] }

/** Host binaries win; otherwise borrow them from postgres:16-alpine. */
export async function clientFor(ctx: Ctx, bin: string): Promise<PgClient> {
  if (await commandExists(ctx, bin)) return { kind: 'host', bin, pre: [] }
  return { kind: 'docker', pre: ['run', '--rm', '-i', '--network', 'host', pgImage(ctx.env), bin] }
}

/** A client's full argv: `client …extra`. */
export const argvOf = (c: PgClient, extra: string[]): [string, string[]] =>
  c.kind === 'host' ? [c.bin, extra] : ['docker', [...c.pre, ...extra]]

// The field separator for one-shot queries is US (0x1f) rather than a tab:
// a naive split collapses runs of whitespace separators, silently shifting
// every column after an empty one.
const PG_FS = '\x1f'

/** One-shot query, no headers. Returns the row as-is (caller splits). */
export async function pgQuery(ctx: Ctx, url: string, sql: string): Promise<string> {
  const c = await clientFor(ctx, 'psql')
  const r = await ctx.exec(...argvOf(c, [url, '-v', 'ON_ERROR_STOP=1', '-At', '-F', PG_FS, '-q', '-c', sql]))
  return r.stdout.trimEnd()
}

// ── Where the blobs live ─────────────────────────────────────────────────────

export type Storage = {
  mode: 'local' | 'internal' | 's3'
  endpoint: string
  bucket: string
  prefix: string
  accessKey: string
  secretKey: string
}

const localStorage = (): Storage => ({ mode: 'local', endpoint: '', bucket: '', prefix: '', accessKey: '', secretKey: '' })

/** The bundled object-storage container, resolved exactly as storage.ts's
 *  internalTarget() does — same env vars, same defaults, empty prefix. */
export function internalTarget(env: Env): Storage {
  return {
    mode: 'internal',
    endpoint: env.TALARIA_S3_URL || `http://127.0.0.1:${env.TALARIA_STORAGE_PORT || env.TALARIA_MINIO_PORT || STORAGE_PORT}`,
    bucket: env.TALARIA_S3_BUCKET || 'talaria',
    prefix: '',
    accessKey: env.TALARIA_S3_ACCESS_KEY || 'talaria',
    secretKey: env.TALARIA_S3_SECRET_KEY || 'talaria-dev-secret',
  }
}

/** An external bucket's secret key is SEALED in app_settings (secretbox), and
 *  a backup must not need the app's decryption keys to run — otherwise the
 *  backup path becomes a second way to unwrap every secret you own. So the
 *  operator hands it over out of band. The access key id is readable from the
 *  row at backup time, but at restore time the database isn't there to read
 *  it from — hence both env vars. */
function withExternalCreds(ctx: Ctx, st: Storage, env: Env, accessKeyFromDb: string): Storage {
  const secret = env.TALARIA_BACKUP_S3_SECRET_KEY
  if (!secret) {
    ctx.log.die(
      'storage mode is "s3" — set TALARIA_BACKUP_S3_SECRET_KEY ' +
        '(the bucket secret is sealed at rest; see docs/BACKUPS.md)',
    )
  }
  const access = accessKeyFromDb || env.TALARIA_BACKUP_S3_ACCESS_KEY
  if (!access) ctx.log.die('storage mode is "s3" — set TALARIA_BACKUP_S3_ACCESS_KEY')
  return { ...st, accessKey: access, secretKey: secret }
}

/** A backup reads the live config out of the database. Two failures kept
 *  apart: a missing app_settings means the app never wrote a storage config
 *  (local is right), an unreachable database means the whole run is wrong.
 *  Collapsing them would silently back up zero blobs and call it a success. */
export async function storageFromDb(ctx: Ctx, dbUrl: string, env: Env): Promise<Storage> {
  let probe: string
  try {
    probe = await pgQuery(ctx, dbUrl, `select to_regclass('public.app_settings') is not null`)
  } catch {
    return ctx.log.die(`cannot query ${dbLabel(dbUrl)} — can't tell where the upload blobs live`)
  }
  if (probe !== 't') return localStorage()
  // No row = the app never left the local default.
  const row = await pgQuery(
    ctx,
    dbUrl,
    `select coalesce(value->>'mode','local'), coalesce(value->>'endpoint',''), coalesce(value->>'bucket',''), coalesce(value->>'prefix',''), coalesce(value->>'accessKeyId','') from app_settings where key = 'storage_config'`,
  )
  if (!row) return localStorage()
  const [mode, endpoint, bucket, prefix, accessKeyId] = row.split(PG_FS)
  const st: Storage = { ...localStorage(), mode: 's3', endpoint, bucket, prefix, accessKey: accessKeyId ?? '' }
  switch (mode) {
    case 'local':
      return localStorage()
    case 'internal':
      return internalTarget(env)
    case 's3':
      return withExternalCreds(ctx, st, env, st.accessKey)
    default:
      return ctx.log.die(`unknown storage mode "${mode}" in app_settings`)
  }
}

/** A restore reads the config out of the snapshot's manifest (the database
 *  may not exist yet). The TALARIA_BACKUP_S3_* overrides point a restore
 *  somewhere else — a drill bucket, or a new provider being migrated to. */
export function storageFromManifest(ctx: Ctx, manifest: string, env: Env): Storage {
  const mode = manifestGet(manifest, 'storage_mode')
  switch (mode) {
    case 'local':
      return localStorage()
    case 'internal':
      return internalTarget(env)
    case 's3': {
      const st: Storage = {
        ...localStorage(),
        mode: 's3',
        endpoint: env.TALARIA_BACKUP_S3_ENDPOINT || manifestGet(manifest, 'storage_endpoint'),
        bucket: env.TALARIA_BACKUP_S3_BUCKET || manifestGet(manifest, 'storage_bucket'),
        prefix: env.TALARIA_BACKUP_S3_PREFIX ?? manifestGet(manifest, 'storage_prefix'),
        accessKey: '',
        secretKey: '',
      }
      return withExternalCreds(ctx, st, env, '')
    }
    default:
      return ctx.log.die('manifest has no usable storage_mode')
  }
}

// ── Manifest ─────────────────────────────────────────────────────────────────

/** First `^key=` value from a manifest — `grep ^k= | cut -d= -f2-`. */
export function manifestGet(manifest: string, key: string): string {
  return envValue(manifest, key) ?? ''
}

// ── rclone, against the resolved bucket ──────────────────────────────────────

/** Every blob key the app writes lives under "<prefix>uploads/" — the Rust
 *  upload store (api/src/uploads.rs). */
export const bucketUploadsPath = (st: Storage): string => `t:${st.bucket}/${st.prefix}uploads`

/** Where the api's local-disk blobs live, for the backup READ: the env pin
 *  first (the same TALARIA_UPLOADS_DIR the api itself reads), then the dev
 *  topology's fixed spot — the api serves from <root>/api and its
 *  cwd-relative default lands at api/.uploads — then the TS-era legacy dir,
 *  for installs old enough to still have blobs there. */
export function localUploadsDir(ctx: Ctx, env: Env): string {
  if (env.TALARIA_UPLOADS_DIR) return env.TALARIA_UPLOADS_DIR
  const api = join(ctx.root, 'api/.uploads')
  if (existsSync(api)) return api
  return join(ctx.root, 'ui/.uploads')
}

/** Per-app Postgres volumes + compose files. Default `app-data/` at the repo
 *  root, same as the UI process (`TALARIA_APP_DATA_DIR`). */
export function localAppDataDir(ctx: Ctx, env: Env): string {
  return env.TALARIA_APP_DATA_DIR || join(ctx.root, 'app-data')
}


/** The remote "t", configured ENTIRELY through env: rclone reads
 *  RCLONE_CONFIG_<REMOTE>_<KEY>, so there is no alias step and no config file
 *  to write. That is what lets the host path and the container path run the
 *  same argv — mc needed `alias set` first, which is why its container form
 *  had to be a single `sh -c` pair sharing one lifetime. RCLONE_CONFIG points
 *  at /dev/null so a missing ~/.rclone.conf does not print a NOTICE over
 *  every backup. */
function rcloneEnv(st: Storage): Record<string, string> {
  return {
    RCLONE_CONFIG: '/dev/null',
    RCLONE_CONFIG_T_TYPE: 's3',
    RCLONE_CONFIG_T_PROVIDER: 'Other',
    RCLONE_CONFIG_T_ENDPOINT: st.endpoint,
    RCLONE_CONFIG_T_ACCESS_KEY_ID: st.accessKey,
    RCLONE_CONFIG_T_SECRET_ACCESS_KEY: st.secretKey,
    RCLONE_CONFIG_T_REGION: 'us-east-1',
  }
}

/** Run an rclone argv against the resolved bucket. A host rclone runs it
 *  directly; without one it runs in a throwaway container on the host network
 *  with the host dir mounted at the SAME path, so one argv works either way.
 *  Credentials travel as env rather than through a shell — there is no shell
 *  here at all any more, so nothing depends on quoting. Returns false on
 *  failure.
 *
 *  `copy`, never `sync`: mc's `mirror` did not delete extras at the
 *  destination and `rclone sync` does. A backup must not be able to empty the
 *  bucket it is reading. */
export async function rcloneRun(ctx: Ctx, dir: string, st: Storage, args: string[]): Promise<boolean> {
  const env = rcloneEnv(st)
  Object.assign(ctx.env, env)
  try {
    if (await commandExists(ctx, 'rclone')) {
      await ctx.exec('rclone', args, { timeoutMs: 7_200_000 })
    } else {
      const uid = process.getuid?.()
      const user = uid === undefined ? [] : ['--user', `${uid}:${process.getgid?.() ?? uid}`]
      const pass = Object.keys(env).flatMap((k) => ['-e', k])
      await ctx.exec(
        'docker',
        ['run', '--rm', '--network', 'host', ...user, ...pass, '-v', `${dir}:${dir}`, rcloneImage(ctx), ...args],
        { timeoutMs: 7_200_000 },
      )
    }
    return true
  } catch {
    return false
  }
}

// ── Checksums ────────────────────────────────────────────────────────────────

async function sha256(file: string): Promise<string> {
  return new Promise((resolve, reject) => {
    const h = createHash('sha256')
    createReadStream(file)
      .on('data', (c) => h.update(c))
      .on('error', reject)
      .on('end', () => resolve(h.digest('hex')))
  })
}

/** Write SHA256SUMS in GNU sha256sum format, so the file stays verifiable by
 *  the host tool too. Streaming hash: dumps outgrow any buffer worth holding. */
export async function writeSums(dir: string, files: string[]): Promise<void> {
  const lines: string[] = []
  for (const f of files) lines.push(`${await sha256(join(dir, f))}  ${f}`)
  // Written LAST, after every byte it covers is on disk — a checksum file
  // that predates its inputs is a lie.
  writeFileSync(join(dir, 'SHA256SUMS'), `${lines.join('\n')}\n`)
}

/** Verify a snapshot's SHA256SUMS. Throws (plain Error) on any mismatch,
 *  missing file, or missing sums — callers turn it into their die(). */
export async function verifySums(dir: string): Promise<void> {
  const sumsPath = join(dir, 'SHA256SUMS')
  if (!existsSync(sumsPath)) throw new Error('no SHA256SUMS')
  for (const line of readFileSync(sumsPath, 'utf8').split('\n')) {
    if (!line) continue
    const m = /^([0-9a-f]{64})  (\S+)$/.exec(line)
    if (!m) throw new Error(`unparseable SHA256SUMS line: ${line}`)
    if (m[2]!.includes('/')) throw new Error(`unexpected path in SHA256SUMS: ${m[2]}`)
    const actual = await sha256(join(dir, m[2]!))
    if (actual !== m[1]) throw new Error(`checksum mismatch: ${m[2]}`)
  }
}

// ── Small formats ────────────────────────────────────────────────────────────

/** The snapshot directory name: the UTC minute the run started. */
export function stampOf(d: Date): string {
  return d.toISOString().replace(/[-:]/g, '').replace(/\.\d{3}/, '')
}

/** `date -u +%FT%TZ` — seconds precision, for the manifest. */
export const isoSecond = (d: Date): string => d.toISOString().replace(/\.\d{3}/, '')

/** `du -h`-shaped size, without spawning du. */
export function humanSize(bytes: number): string {
  for (const [unit, div] of [
    ['G', 1024 ** 3],
    ['M', 1024 ** 2],
    ['K', 1024],
  ] as const) {
    if (bytes >= div) return `${(bytes / div).toFixed(1)}${unit}`
  }
  return `${bytes}`
}
