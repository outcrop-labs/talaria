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

import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import type { Ctx } from '../ctx'
import type { Leaf } from '../cli'
import { argvOf, bucketUploadsPath, clientFor, dbLabel, liftAppEnv, manifestGet, mcRun, pgQuery, storageFromManifest, verifySums } from '../backup/lib'

export type RestoreWhat = 'all' | 'db' | 'uploads'

export async function runRestore(
  ctx: Ctx,
  snap: string,
  opts: { target?: string; what: RestoreWhat; yes?: boolean },
): Promise<number> {
  snap = snap.replace(/\/+$/, '')
  const manifestPath = join(snap, 'manifest.txt')
  if (!existsSync(manifestPath)) ctx.log.die(`${snap} is not a Talaria snapshot (no manifest.txt)`)
  const manifest = readFileSync(manifestPath, 'utf8')

  const env = liftAppEnv(ctx)
  const target = opts.target || env.DATABASE_URL
  if (!target) ctx.log.die('no target database — pass --target, or set DATABASE_URL')

  ctx.log.say(`Verifying ${snap}`)
  await verifySums(snap).catch(() => ctx.log.die('checksum mismatch — this snapshot is corrupt, use an older one'))
  ctx.log.ok(`checksums match (taken ${manifestGet(manifest, 'created_at')} from ${manifestGet(manifest, 'database')})`)

  // Confirmation is not optional theatre here: the usual way to lose data is
  // to restore a good snapshot over the wrong database.
  const whatLabel = opts.what === 'all' ? 'the database and the upload blobs' : `the ${opts.what}`
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
        if (!(await mcRun(ctx, tmp, st, ['mirror', '--quiet', '--overwrite', tmp, bucketUploadsPath(st)]))) {
          ctx.log.die(`could not mirror into ${bucketUploadsPath(st)} — check the endpoint and credentials`)
        }
      } finally {
        rmSync(tmp, { recursive: true, force: true })
      }
      ctx.log.ok(`uploaded to bucket ${st.bucket} at ${st.endpoint}`)
    }
  }

  ctx.log.raw(`
Restored.

  Start the app, sign in, and open a message attachment — that exercises
  both halves (row in Postgres, bytes in storage) in one click.
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
