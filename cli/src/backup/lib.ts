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
import { envValue, envWins, parseEnv, stripQuotes } from '../envfile'

// Client images, only used when the host has no psql/pg_dump/mc. postgres:16
// matches docker/dev-compose.yml — a dump is refused if the client is older
// than the server, so bump this together with the server image.
export const pgImage = (env: Env): string => env.TALARIA_PG_IMAGE || 'postgres:16-alpine'
export const mcImage = (env: Env): string => env.TALARIA_MC_IMAGE || 'minio/mc:latest'

type Env = Record<string, string | undefined>

// ── App config ───────────────────────────────────────────────────────────────

/** The runtime view of the app's env: ui/.env (or $TALARIA_ENV_FILE) with the
 *  shell ALWAYS winning, so an operator can back up a remote instance without
 *  a checkout of its .env. Env wins even when empty — the documented footgun;
 *  an empty DATABASE_URL export is a loud error, not a silent fallback. */
export function liftAppEnv(ctx: Ctx): Env {
  const file = join(ctx.root, ctx.env.TALARIA_ENV_FILE || 'ui/.env')
  const text = existsSync(file) ? readFileSync(file, 'utf8') : ''
  // One layer of quotes tolerated: the bash stripped them for files other
  // tools had written by hand.
  const parsed: Record<string, string> = {}
  for (const [k, v] of Object.entries(parseEnv(text))) parsed[k] = stripQuotes(v)
  return envWins(parsed, ctx.env)
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

/** The bundled MinIO container, resolved exactly as storage.ts's
 *  internalTarget() does — same env vars, same defaults, empty prefix. */
export function internalTarget(env: Env): Storage {
  return {
    mode: 'internal',
    endpoint: env.TALARIA_S3_URL || `http://127.0.0.1:${env.TALARIA_MINIO_PORT || '9010'}`,
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

// ── mc, against the resolved bucket ──────────────────────────────────────────

/** Every blob key the app writes lives under "<prefix>uploads/" — the Rust
 *  upload store (api/src/uploads.rs). */
export const bucketUploadsPath = (st: Storage): string => `t/${st.bucket}/${st.prefix}uploads`

/** Single-quote for the one place a shell string is unavoidable (the docker
 *  mc one-shot: alias set + the command must share one container lifetime). */
const shq = (s: string): string => `'${s.replaceAll("'", `'\\''`)}'`

/** Run an `mc` argv (after `--config-dir`) against the resolved bucket. Host
 *  mc runs as two plain invocations (the alias persists in /tmp/mc-talaria);
 *  without a host mc, both run inside ONE throwaway container — the alias
 *  setup and the command must share a container lifetime, so there the pair
 *  is a single sh -c, with credentials passed as env (never interpolated into
 *  the script). The host dir is mounted at the SAME path inside so one argv
 *  works either way. Returns false on failure (callers distinguish an empty
 *  prefix from an unreachable bucket by retrying with `ls`). */
export async function mcRun(ctx: Ctx, dir: string, st: Storage, args: string[]): Promise<boolean> {
  ctx.env.S3_ENDPOINT = st.endpoint
  ctx.env.S3_ACCESS_KEY = st.accessKey
  ctx.env.S3_SECRET_KEY = st.secretKey
  const dirArg = ['--config-dir', '/tmp/mc-talaria']
  try {
    if (await commandExists(ctx, 'mc')) {
      await ctx.exec('mc', [...dirArg, 'alias', 'set', 't', st.endpoint, st.accessKey, st.secretKey])
      await ctx.exec('mc', [...dirArg, ...args], { timeoutMs: 7_200_000 })
    } else {
      const uid = process.getuid?.()
      const user = uid === undefined ? [] : ['--user', `${uid}:${process.getgid?.() ?? uid}`]
      const script =
        `mc --config-dir /tmp/mc-talaria alias set t "$S3_ENDPOINT" "$S3_ACCESS_KEY" "$S3_SECRET_KEY" >/dev/null && ` +
        `mc --config-dir /tmp/mc-talaria ${args.map(shq).join(' ')}`
      await ctx.exec(
        'docker',
        [
          'run', '--rm', '--network', 'host', ...user,
          '-e', 'S3_ENDPOINT', '-e', 'S3_ACCESS_KEY', '-e', 'S3_SECRET_KEY', '-e', 'HOME=/tmp',
          '-v', `${dir}:${dir}`, '--entrypoint', 'sh', mcImage(ctx.env), '-c', script,
        ],
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
