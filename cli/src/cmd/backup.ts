// `talaria backup` — one snapshot of the two things that cannot be rebuilt:
// the Postgres database and the upload blobs (local disk, the bundled MinIO
// bucket, or an external S3 bucket — whichever Admin → Storage is using).
// Port of scripts/backup.sh.
//
// Redis is deliberately NOT backed up: it holds sessions and ephemeral state,
// so losing it signs everyone out and nothing more. Qdrant isn't either —
// every vector is re-derivable from Postgres by reindexing.
//
// Nothing here schedules itself; point cron/systemd at it. Retention + the
// RESTORE procedure: docs/BACKUPS.md.

import { closeSync, existsSync, mkdirSync, openSync, readSync, readdirSync, renameSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { hostname } from 'node:os'
import { join } from 'node:path'
import type { Ctx } from '../ctx'
import type { Leaf } from '../cli'
import {
  argvOf, bucketUploadsPath, clientFor, dbLabel, humanSize, isoSecond, liftAppEnv, localUploadsDir, mcRun,
  stampOf, storageFromDb, writeSums,
} from '../backup/lib'
import { canonicalDir } from '../paths'

/** The last `n` bytes of a file, as text — pg_dump writes its completion
 *  trailer only when it actually finished, so the tail of the plain dump is
 *  the truncation check (fs only; no `gunzip | tail` pipeline to capture). */
function tailOf(file: string, n: number): string {
  const size = statSync(file).size
  const len = Math.min(n, size)
  const buf = Buffer.alloc(len)
  const fd = openSync(file, 'r')
  try {
    readSync(fd, buf, 0, len, size - len)
  } finally {
    closeSync(fd)
  }
  return buf.toString('utf8')
}

/** Retention, on its own so it is testable: snapshot directory names sort
 *  chronologically. Only directories that carry a manifest are ours at all —
 *  a stray directory in the backup folder is neither deleted nor counted
 *  against the budget (in the bash, a stray dir silently meant one fewer real
 *  snapshot kept). Returns how many were removed. */
export function pruneSnapshots(dest: string, keep: number): number {
  if (keep <= 0) return 0
  const ours = readdirSync(dest, { withFileTypes: true })
    .filter((e) => e.isDirectory() && /Z$/.test(e.name) && existsSync(join(dest, e.name, 'manifest.txt')))
    .map((e) => e.name)
    .sort()
  let pruned = 0
  for (const name of ours.slice(0, ours.length - keep)) {
    rmSync(join(dest, name), { recursive: true, force: true })
    pruned++
  }
  return pruned
}

export async function runBackup(ctx: Ctx, dest: string, keep: number): Promise<number> {
  const dockerOrPg = async (): Promise<boolean> => {
    for (const bin of ['docker', 'pg_dump']) {
      try {
        await ctx.exec(bin, ['--version'])
        return true
      } catch {
        /* try the next */
      }
    }
    return false
  }
  if (!(await dockerOrPg())) ctx.log.die('need either pg_dump on PATH or docker')
  const env = liftAppEnv(ctx)
  const dbUrl = env.DATABASE_URL
  if (!dbUrl) ctx.log.die(`DATABASE_URL is not set (looked in the environment and ${ctx.env.TALARIA_ENV_FILE || 'ui/.env'})`)

  // Everything is written to <stamp>.partial and renamed at the very end, so
  // a half-written snapshot never looks complete and retention can never
  // delete a good one in favour of a broken one.
  const snap = join(dest, stampOf(ctx.now()))
  const stage = `${snap}.partial`
  if (existsSync(snap)) ctx.log.die(`${snap} already exists`)
  mkdirSync(dest, { recursive: true })
  // A snapshot is a full database dump; the default destination sits inside
  // the checkout. Make the directory ignore itself rather than trusting that
  // whichever repo it lands in has a rule for it.
  if (!existsSync(join(dest, '.gitignore'))) writeFileSync(join(dest, '.gitignore'), '*\n')
  mkdirSync(stage, { recursive: true })
  const stageAbs = canonicalDir(stage) // mc mounts it into a container: it needs the real path

  try {
    ctx.log.say(`Postgres → db.sql.gz  (${dbLabel(dbUrl)})`)
    // --no-owner/--no-privileges so the dump restores under whatever role
    // does the restoring; --clean --if-exists so it lands on a non-empty
    // database too. -f (the client writes the file itself) rather than a
    // redirect, so the dump never occupies a pipe or a buffer — in the docker
    // client shape the staging dir is mounted at the same path so -f resolves.
    const plain = join(stageAbs, 'db.sql')
    const client = await clientFor(ctx, 'pg_dump')
    // In the docker shape the staging dir is mounted at the same path so the
    // client's `-f` resolves inside the container too. pre ends [image, bin]
    // and docker flags must precede the image, so splice before the last two.
    const mounted = client.kind === 'docker'
      ? { ...client, pre: [...client.pre.slice(0, -2), '-v', `${stageAbs}:${stageAbs}`, ...client.pre.slice(-2)] }
      : client
    await ctx.exec(
      ...argvOf(mounted, [dbUrl, '--clean', '--if-exists', '--no-owner', '--no-privileges', '-f', plain]),
      { timeoutMs: 600_000 },
    ).catch((e) => ctx.log.die(`pg_dump failed: ${e instanceof Error ? e.message : String(e)}`))
    // A truncated dump is the classic silent backup failure (a full disk, an
    // OOM-killed client, a connection dropped mid-stream). Check the trailer
    // on the PLAIN file while it still exists, then compress and re-check the
    // archive itself.
    if (!tailOf(plain, 512).includes('PostgreSQL database dump complete')) {
      ctx.log.die('the dump has no completion marker — it was truncated, refusing to keep it')
    }
    const gz = `${plain}.gz`
    await ctx.exec('gzip', ['-k', plain]).catch(() => ctx.log.die('gzip failed'))
    await ctx.exec('gzip', ['-t', gz]).catch(() => ctx.log.die('the dump is not a valid gzip archive'))
    rmSync(plain)
    ctx.log.ok(`${humanSize(statSync(gz).size)} compressed`)

    ctx.log.say('Upload blobs')
    const st = await storageFromDb(ctx, dbUrl, env)
    let uploadsDir = ''
    const tarPath = join(stageAbs, 'uploads.tar.gz')
    if (st.mode === 'local') {
      uploadsDir = localUploadsDir(ctx, env)
      if (existsSync(uploadsDir)) {
        await ctx.run('tar', ['-czf', tarPath, '-C', uploadsDir, '.'])
        ctx.log.ok(`local disk — ${uploadsDir}`)
      } else {
        await ctx.run('tar', ['-czf', tarPath, '-T', '/dev/null'])
        ctx.log.warn(`local disk — ${uploadsDir} does not exist yet (empty archive written)`)
      }
    } else {
      mkdirSync(join(stageAbs, 'blobs'), { recursive: true })
      const src = bucketUploadsPath(st)
      if (
        !(await mcRun(ctx, stageAbs, st, ['mirror', '--quiet', '--overwrite', src, join(stageAbs, 'blobs')]))
      ) {
        // An empty prefix is not an error — but an unreachable bucket is, and
        // mc reports both by exiting non-zero. Tell them apart before
        // continuing.
        if (!(await mcRun(ctx, stageAbs, st, ['ls', `t/${st.bucket}`]))) {
          ctx.log.die(`bucket ${st.bucket} at ${st.endpoint} is unreachable — check the endpoint and credentials`)
        }
        ctx.log.warn(`no objects under ${src} yet`)
      }
      await ctx.run('tar', ['-czf', tarPath, '-C', join(stageAbs, 'blobs'), '.'])
      rmSync(join(stageAbs, 'blobs'), { recursive: true, force: true }) // the tar is the artifact; the mirror was scratch
      ctx.log.ok(`bucket ${st.bucket} at ${st.endpoint} (${st.mode})`)
    }
    const blobs = (await ctx.exec('tar', ['-tzf', tarPath])).stdout
      .split('\n')
      .filter((l) => l && !l.endsWith('/')).length
    ctx.log.ok(`${blobs} blob(s), ${humanSize(statSync(tarPath).size)} compressed`)

    // Identifiers only — never credentials. `talaria restore` reads the
    // storage fields back out of here, because at restore time the database
    // isn't there to ask.
    writeFileSync(
      join(stageAbs, 'manifest.txt'),
      [
        'talaria_backup=1',
        `created_at=${isoSecond(ctx.now())}`,
        `created_on=${hostname()}`,
        `database=${dbLabel(dbUrl)}`,
        `storage_mode=${st.mode}`,
        `storage_endpoint=${st.endpoint}`,
        `storage_bucket=${st.bucket}`,
        `storage_prefix=${st.prefix}`,
        `uploads_dir=${uploadsDir}`,
        `blob_count=${blobs}`,
      ].join('\n') + '\n',
    )
    await writeSums(stageAbs, ['db.sql.gz', 'uploads.tar.gz', 'manifest.txt'])

    renameSync(stage, snap)
    const total = statSync(join(snap, 'db.sql.gz')).size + statSync(join(snap, 'uploads.tar.gz')).size
    ctx.log.ok(`snapshot complete — ${snap} (${humanSize(total)})`)

    if (keep > 0) {
      ctx.log.say(`Retention (keeping the newest ${keep})`)
      ctx.log.ok(`${pruneSnapshots(dest, keep)} older snapshot(s) removed`)
    }
  } catch (e) {
    // The bash's `trap rm -rf STAGE EXIT`: whatever went wrong, the partial
    // directory must not survive to look like a snapshot.
    rmSync(stage, { recursive: true, force: true })
    throw e
  }

  ctx.log.raw(`
Backed up.

  Restore it:   bun talaria restore ${snap} --target <postgres-url>
  Procedure:    docs/BACKUPS.md   (test a restore before you need one)
`)
  return 0
}

export const backupCommand: Leaf = {
  kind: 'leaf',
  name: 'backup',
  summary: 'snapshot the database + upload blobs (Postgres dump + blobs, staged atomically)',
  usage: 'talaria backup [dest] [--keep N]',
  positionals: { name: 'dest', desc: 'snapshot directory (default: $TALARIA_BACKUP_DIR or backups)' },
  flags: [
    { name: 'keep', kind: 'value', desc: 'snapshots to keep, 0 disables pruning (default: $TALARIA_BACKUP_KEEP or 7)' },
  ],
  run: (ctx, args) => {
    const rawKeep = args.flags.keep !== undefined ? String(args.flags.keep) : (ctx.env.TALARIA_BACKUP_KEEP ?? '7')
    const keep = Number(rawKeep)
    if (!Number.isInteger(keep) || keep < 0) ctx.log.die(`--keep must be a non-negative integer, got \`${rawKeep}\``)
    return runBackup(ctx, args.positionals[0] || ctx.env.TALARIA_BACKUP_DIR || 'backups', keep)
  },
}
