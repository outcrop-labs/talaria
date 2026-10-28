// `talaria restore` — put a snapshot back. DESTRUCTIVE: the dump drops and
// recreates every object it owns in the target database. Port of
// scripts/restore.sh.
//
// Stop the app first — it runs migrations on its first query and holds a pool
// open, and neither survives the schema being swapped underneath it.
//
// The target defaults to DATABASE_URL (environment, then ui/.env). Always
// name --target explicitly when restoring somewhere other than this
// checkout's app. Full procedure, including the drill you should actually
// run: docs/BACKUPS.md.

import { chmodSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import type { Ctx } from '../ctx'
import type { Leaf } from '../cli'
import { argvOf, bucketUploadsPath, clientFor, dbLabel, liftAppEnv, localAppDataDir, manifestGet, pgQuery, rcloneRun, storageFromManifest, verifySums } from '../backup/lib'
import { readEnvFile } from '../envfile'
import { inside, refuseForeignCheckout } from '../stack'


export type RestoreWhat = 'all' | 'db' | 'uploads'


function chmodDbEnvs(dir: string): void {
  if (!existsSync(dir)) return
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, e.name)
    if (e.isDirectory()) chmodDbEnvs(p)
    else if (e.name === 'db.env') chmodSync(p, 0o600)
  }
}

/** Where a restore inside a worktree would write somewhere that is not the
 *  worktree's own, or null when every target is. Whether this IS a worktree
 *  is read from the checkout's own ui/.env, never from TALARIA_ENV_FILE: an
 *  env file pointed at main's would otherwise switch every rule below off. A
 *  worktree owns its database, its checkout's dirs and nothing else:
 *
 *   · a DATABASE_URL in the shell, or in a TALARIA_ENV_FILE, that is not
 *     ui/.env's, with no --target, leaves which database is meant a guess;
 *   · blobs in internal mode would land in the main stack's object storage,
 *     which a worktree's app shares (it has none of its own), and in s3 mode in
 *     whatever bucket the snapshot names, unless the operator names one
 *     (TALARIA_BACKUP_S3_BUCKET / _ENDPOINT);
 *   · an uploads or app-data dir outside the checkout is inherited from main's
 *     env and is main's.
 *
 *  Pure apart from reading the manifest's mode, so each rule is testable. */
export function worktreeRestoreProblem(
  ctx: Ctx,
  env: Record<string, string | undefined>,
  manifest: string,
  opts: { target?: string; what: RestoreWhat },
): string | null {
  // Quotes stripped, as liftAppEnv reads the same files, so one value in two
  // spellings is not a conflict.
  const ownEnv = readEnvFile(ctx, 'ui/.env', { quotes: true })
  const worktree = ownEnv.TALARIA_WORKTREE
  if (!worktree) return null
  const own = `this is worktree "${worktree}"`
  const ownDb = ownEnv.DATABASE_URL
  const ownLabel = ownDb ? dbLabel(ownDb) : 'unset'
  if (!opts.target && ctx.env.DATABASE_URL !== undefined && ctx.env.DATABASE_URL !== ownDb) {
    return (
      `${own}: DATABASE_URL in the environment (${dbLabel(ctx.env.DATABASE_URL)}) is not its ui/.env's ` +
      `(${ownLabel}). Pass --target to say which database you mean, or unset DATABASE_URL.`
    )
  }
  // The shell wins over the env file (liftAppEnv), so the file's DATABASE_URL
  // is the target only when the shell names none.
  const envFile = ctx.env.TALARIA_ENV_FILE
  if (!opts.target && envFile && ctx.env.DATABASE_URL === undefined) {
    const fileDb = readEnvFile(ctx, envFile, { quotes: true }).DATABASE_URL
    if (fileDb !== ownDb) {
      return (
        `${own}: TALARIA_ENV_FILE=${envFile} names DATABASE_URL ${fileDb ? dbLabel(fileDb) : '(none)'}, not its ui/.env's ` +
        `(${ownLabel}). Pass --target to say which database you mean, or unset TALARIA_ENV_FILE.`
      )
    }
  }
  if (opts.what !== 'db') {
    const mode = manifestGet(manifest, 'storage_mode')
    if (mode === 'internal') {
      return (
        `${own}, and it has no object storage of its own: its app keeps blobs in the main stack's. ` +
        'Restoring the blobs here would write into main\'s bucket. Use --db-only, or restore the blobs from the checkout that owns that storage.'
      )
    }
    if (mode === 's3' && !ctx.env.TALARIA_BACKUP_S3_BUCKET && !ctx.env.TALARIA_BACKUP_S3_ENDPOINT) {
      return (
        `${own}: the snapshot's blobs go to bucket ${manifestGet(manifest, 'storage_bucket') || '(unnamed)'}, which is the bucket ` +
        'its source used and not this worktree\'s. Name the bucket with TALARIA_BACKUP_S3_BUCKET (and _ENDPOINT), or use --db-only.'
      )
    }
    if (mode === 'local') {
      // resolve() is cwd-relative, as the extract below is.
      const dir = resolve(env.TALARIA_UPLOADS_DIR || join(ctx.root, 'api/.uploads'))
      if (!inside(ctx.root, dir)) {
        return `${own}, but the uploads dir ${dir} is outside it (TALARIA_UPLOADS_DIR, inherited from main's env). Unset it, or use --db-only.`
      }
    }
  }
  if (opts.what === 'all') {
    const dir = resolve(localAppDataDir(ctx, env))
    if (!inside(ctx.root, dir)) {
      return `${own}, but the app-data dir ${dir} is outside it (TALARIA_APP_DATA_DIR, inherited from main's env). Unset it, or use --db-only.`
    }
  }
  return null
}

export async function runRestore(
  ctx: Ctx,
  snap: string,
  opts: { target?: string; what: RestoreWhat; yes?: boolean },
): Promise<number> {
  // Run through the shim from another checkout, the default target and every
  // dir below would be the shim's checkout's.
  refuseForeignCheckout(ctx, 'restore')
  snap = snap.replace(/\/+$/, '')
  const manifestPath = join(snap, 'manifest.txt')
  if (!existsSync(manifestPath)) ctx.log.die(`${snap} is not a Talaria snapshot (no manifest.txt)`)
  const manifest = readFileSync(manifestPath, 'utf8')

  const env = liftAppEnv(ctx)
  const problem = worktreeRestoreProblem(ctx, env, manifest, opts)
  if (problem) ctx.log.die(`${problem}\n  Nothing was changed.`)
  const target = opts.target || env.DATABASE_URL
  if (!target) ctx.log.die('no target database — pass --target, or set DATABASE_URL')

  ctx.log.say(`Verifying ${snap}`)
  await verifySums(snap).catch(() => ctx.log.die('checksum mismatch — this snapshot is corrupt, use an older one'))
  ctx.log.ok(`checksums match (taken ${manifestGet(manifest, 'created_at')} from ${manifestGet(manifest, 'database')})`)

  // Confirmation is not optional theatre here: the usual way to lose data is
  // to restore a good snapshot over the wrong database.
  const whatLabel = opts.what === 'all' ? 'the database, the upload blobs, and the app databases' : `the ${opts.what}`
  ctx.log.raw(`\n  This will REPLACE ${whatLabel} in \x1b[1m${dbLabel(target)}\x1b[0m\n\n`)

  if (!opts.yes) {
    if (!ctx.isTTY) ctx.log.die('refusing to restore non-interactively without --yes')
    const reply = await ctx.readLine('  Type \'restore\' to continue: ')
    if (reply !== 'restore') ctx.log.die('aborted')
  }

  if (opts.what !== 'uploads') {
    ctx.log.say(`Postgres ← ${snap}/db.sql.gz`)
    // ON_ERROR_STOP so a partial restore is a failure, not a warning
    // scrolling by. The restore STREAMS (gunzip | psql) — never buffered.
    const client = await clientFor(ctx, 'psql')
    try {
      await ctx.pipe(
        ['gunzip', ['-c', join(snap, 'db.sql.gz')]],
        argvOf(client, [target, '-v', 'ON_ERROR_STOP=1', '-q', '-o', '/dev/null']),
      )
    } catch (e) {
      ctx.log.die(
        `restore failed — the target database is now in an incomplete state, fix the cause and re-run ` +
          `(${e instanceof Error ? e.message : String(e)})`,
      )
    }
    const tables = await pgQuery(ctx, target, `select count(*) from information_schema.tables where table_schema='public'`)
    ctx.log.ok(`${tables} tables restored`)
  }

  if (opts.what !== 'db') {
    ctx.log.say(`Upload blobs ← ${snap}/uploads.tar.gz`)
    const st = storageFromManifest(ctx, manifest, env)
    if (st.mode === 'local') {
      // The snapshot's uploads_dir is where they lived on the SOURCE host;
      // this host's config decides where they land. Restore WRITES to the
      // api's dir (env pin, else the dev topology's api/.uploads) — never the
      // TS-era legacy path the backup read may still fall back to.
      const uploadsDir = env.TALARIA_UPLOADS_DIR || join(ctx.root, 'api/.uploads')
      mkdirSync(uploadsDir, { recursive: true })
      await ctx.run('tar', ['-xzf', join(snap, 'uploads.tar.gz'), '-C', uploadsDir])
      ctx.log.ok(`extracted to ${uploadsDir}`)
    } else {
      const tmp = mkdtempSync(join(tmpdir(), 'talaria-restore-'))
      try {
        await ctx.run('tar', ['-xzf', join(snap, 'uploads.tar.gz'), '-C', tmp])
        if (!(await rcloneRun(ctx, tmp, st, ['copy', tmp, bucketUploadsPath(st)]))) {
          ctx.log.die(`could not copy into ${bucketUploadsPath(st)} — check the endpoint and credentials`)
        }
      } finally {
        rmSync(tmp, { recursive: true, force: true })
      }
      ctx.log.ok(`uploaded to bucket ${st.bucket} at ${st.endpoint}`)
    }
  }

  if (opts.what === 'all') {
    const appTar = join(snap, 'app-data.tar.gz')
    if (existsSync(appTar)) {
      const dest = localAppDataDir(ctx, env)
      ctx.log.say(`App databases ← ${appTar}`)
      mkdirSync(dest, { recursive: true })
      await ctx.run('tar', ['-xzf', appTar, '-C', dest])
      chmodDbEnvs(dest)
      ctx.log.ok(`extracted to ${dest}`)
    }
  }

  ctx.log.raw(`
Restored.

  Start the app, sign in, and open a message attachment — that exercises
  the catalog and the blobs in one click. App databases come up on next
  spawn (dump.sql is applied then).
`)
  return 0
}

export const restoreCommand: Leaf = {
  kind: 'leaf',
  name: 'restore',
  summary: 'restore a snapshot (destructive: drops and recreates the target)',
  usage: 'talaria restore <snapshot-dir> [--target <url>] [--db-only|--uploads-only] [--yes]',
  positionals: { name: 'snapshot-dir', required: true, desc: 'a directory `talaria backup` wrote' },
  flags: [
    { name: 'target', kind: 'value', desc: 'postgres URL to restore into (default: DATABASE_URL)' },
    { name: 'db-only', kind: 'bool', desc: 'restore only the database half' },
    { name: 'uploads-only', kind: 'bool', desc: 'restore only the blob half' },
    { name: 'yes', short: 'y', kind: 'bool', desc: 'skip the typed confirm (automation)' },
  ],
  run: (ctx, args) => {
    const snap = args.positionals[0]
    if (!snap) ctx.log.die('usage: talaria restore <snapshot-dir> [--target <url>] [--db-only|--uploads-only] [--yes]')
    if (args.flags['db-only'] && args.flags['uploads-only']) {
      ctx.log.die('--db-only and --uploads-only are mutually exclusive')
    }
    const what: RestoreWhat = args.flags['db-only'] ? 'db' : args.flags['uploads-only'] ? 'uploads' : 'all'
    const target = typeof args.flags.target === 'string' ? args.flags.target : undefined
    return runRestore(ctx, snap, { target, what, yes: args.flags.yes === true })
  },
}
