// `talaria box seed` — seed a devbox with real starter data from the primary
// dev environment. Invoked by `box new`; runnable directly. Port of
// scripts/devbox-seed.sh.
//
// Scope, and why:
//   Postgres   REQUIRED — a point-in-time dump of the primary dev DB (the
//              worktree.sh shape). Everything the app shows comes from here.
//   MinIO      REQUIRED — DB rows reference s3:// blobs; without the mirror
//              the seeded UI shows broken attachments. Box minio runs the
//              same creds as primary, so the mirror is pure bytes.
//   chassis    fleet config — the template with the network repointed at this
//   + fleet/.env  box's private fleet network, and the LLM endpoint copied
//              from the primary fleet/.env (agents need values present; the
//              renderer rewrites them through the box's own gateway anyway).
//   Qdrant     OPTIONAL (--qdrant) — a DERIVED index (vectors of DB-stored
//              docs). Default off: re-run the KB backfill in the box's app
//              instead (the embeddings service is shared, so dimensions
//              match). The flag round-trips an HTTP snapshot.
//   Redis      NEVER — sessions/queues are transient by design.
//
// SNAPSHOT semantics: a seed is a copy, not a link. Later primary changes do
// not flow; re-run with --force to take a fresh copy. Idempotent without it:
// a box that already has tables keeps its data (and your edits to chassis.yml
// / fleet/.env survive — only --force overwrites those two).

import { copyFileSync, existsSync, mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import type { Ctx } from '../../ctx'
import type { Leaf } from '../../cli'
import { envValue, writeSecret } from '../../envfile'
import { boxState } from './shared'

/** Repoint the chassis's fleet network at THIS box's own — the template
 *  ships the primary install's default name. Range-scoped to the network:
 *  block (`sed /"^network:"/,/^[^ ]/`): a second `  name:` elsewhere in the
 *  file must not move. Pure so the scoping is testable. */
export function repointChassis(text: string, network: string): string {
  const out: string[] = []
  let inBlock = false
  for (const line of text.split('\n')) {
    if (/^network:/.test(line)) inBlock = true
    else if (inBlock && /^[^ ]/.test(line)) inBlock = false
    if (inBlock && /^ {2}name: /.test(line)) out.push(`  name: ${network}`)
    else out.push(line)
  }
  return out.join('\n')
}

/** The seeded fleet/.env: header + the endpoint lines agents need present.
 *  Pure so the grep is testable. */
export function seedFleetEnv(primaryFleetEnv: string): string {
  const kept = primaryFleetEnv
    .split('\n')
    .filter((l) => /^(LLM_BASE_URL|LLM_API_KEY|LLM_MODEL|HERMES_IMAGE)=/.test(l))
    .join('\n')
  return `# Fleet env — seeded from the primary fleet/.env by \`talaria box seed\`.
# The renderer rewrites the endpoint through this box's own gateway;
# agents still need the values present to interpolate.
${kept}
`
}

export async function runSeed(ctx: Ctx, name: string, o: { force?: boolean; qdrant?: boolean } = {}): Promise<number> {
  const root = ctx.root
  const state = boxState(ctx, name)
  // NOT requireBox: `new` calls this before it writes box.env.
  const MAIN_PGC = ctx.env.TALARIA_PG_CONTAINER ?? 'talaria-postgres-dev'
  const MAIN_MINIOC = ctx.env.TALARIA_MINIO_CONTAINER ?? 'talaria-minio-dev'
  const MAIN_QDRANT = ctx.env.TALARIA_QDRANT_CONTAINER ?? 'talaria-qdrant-dev'
  const PGC = `devbox-${name}-postgres`

  const up = async (c: string): Promise<boolean> => {
    try {
      await ctx.exec('docker', ['inspect', c])
      return true
    } catch {
      return false
    }
  }
  if (!(await up(PGC))) ctx.log.die(`box postgres (${PGC}) isn't running`)
  if (!(await up(MAIN_PGC))) ctx.log.die(`primary postgres (${MAIN_PGC}) isn't running — start the main stack first`)

  // ── Postgres ───────────────────────────────────────────────────────────────
  ctx.log.say('Postgres — point-in-time copy of the primary dev DB')
  let seeded = false
  try {
    const probe = await ctx.exec('docker', [
      'exec', PGC, 'psql', '-U', 'talaria', '-d', 'talaria', '-tAc',
      "select 1 from information_schema.tables where table_schema='public' limit 1",
    ])
    seeded = probe.stdout.trim().length > 0
  } catch {
    seeded = false
  }
  if (seeded && !o.force) {
    ctx.log.ok('already seeded (box DB has tables) — --force to re-copy')
  } else {
    // STREAMED, never buffered: dumps blow past any maxBuffer in seconds.
    // pipefail both ways — a swallowed pg_dump failure would look like a
    // successful seed. The restore side's stdout is discarded (set_config and
    // friends): chatter, not signal; its stderr still surfaces.
    try {
      await ctx.pipe(
        ['docker', ['exec', MAIN_PGC, 'pg_dump', '-U', 'talaria', '-d', 'talaria', '--clean', '--if-exists']],
        ['docker', ['exec', '-i', PGC, 'psql', '-U', 'talaria', '-d', 'talaria', '-q']],
        { quietDst: true },
      )
    } catch (e) {
      ctx.log.die(`seed dump/restore failed: ${e instanceof Error ? e.message : String(e)}`)
    }
    ctx.log.ok('seeded')
  }

  // ── MinIO ──────────────────────────────────────────────────────────────────
  ctx.log.say('MinIO — mirroring the primary dev bucket (DB rows reference these blobs)')
  const uiEnv = readFileSync(join(root, 'ui/.env'), 'utf8')
  const s3Key = envValue(uiEnv, 'TALARIA_S3_ACCESS_KEY') ?? 'talaria'
  const s3Secret = envValue(uiEnv, 'TALARIA_S3_SECRET_KEY') ?? 'talaria-dev-secret'
  const s3Bucket = envValue(uiEnv, 'TALARIA_S3_BUCKET') ?? 'talaria'
  const DSTC = `devbox-${name}-minio`
  const mirrorFails = 'mirror failed — attachments in the seeded UI will be broken until re-uploaded'
  // The two minios live on different networks (primary dev vs this box's
  // own), so the mirror runs through a throwaway mc container joined to
  // both. Created stopped (networks attach to stopped containers), then
  // started with a sleep PID — the image's `mc` entrypoint with no arguments
  // exits instantly, and exec needs a live container.
  const MCT = `devbox-${name}-seed-mc`
  try {
    await ctx.exec('docker', ['rm', '-f', MCT]).catch(() => {})
    await ctx.exec('docker', ['create', '--name', MCT, '--entrypoint', 'sh', 'docker.io/minio/mc:latest', '-c', 'sleep infinity'])
    await ctx.exec('docker', ['network', 'connect', `devbox-${name}_default`, MCT])
    await ctx.exec('docker', ['network', 'connect', 'talaria-dev_default', MCT])
    await ctx.exec('docker', ['start', MCT])
    // Single quotes guard the creds from the inner sh — dev keys are hex/simple.
    // A throw from the exec IS the failure signal.
    await ctx.exec('docker', [
      'exec', MCT, 'sh', '-c',
      `mc alias set src http://${MAIN_MINIOC}:9000 '${s3Key}' '${s3Secret}' >/dev/null && ` +
        `mc alias set dst http://${DSTC}:9000 '${s3Key}' '${s3Secret}' >/dev/null`,
    ])
    let srcHasBucket = false
    try {
      await ctx.exec('docker', ['exec', MCT, 'sh', '-c', `mc stat src/${s3Bucket} >/dev/null 2>&1`])
      srcHasBucket = true
    } catch {
      srcHasBucket = false
    }
    if (!srcHasBucket) {
      // The app creates the bucket lazily; a primary that never uploaded
      // anything has none. Nothing references a blob, so there is nothing
      // to mirror.
      ctx.log.ok(`primary has no '${s3Bucket}' bucket yet (no uploads ever) — nothing to mirror`)
    } else {
      await ctx.exec('docker', [
        'exec', MCT, 'sh', '-c',
        `mc mb --ignore-existing dst/${s3Bucket} >/dev/null 2>&1; ` +
          `mc mirror --overwrite src/${s3Bucket} dst/${s3Bucket} >/dev/null`,
      ])
      ctx.log.ok('mirrored')
    }
  } catch {
    ctx.log.warn(`couldn't complete the mirror — ${mirrorFails}`)
  } finally {
    await ctx.exec('docker', ['rm', '-f', MCT]).catch(() => {})
  }

  // ── Fleet config plane ─────────────────────────────────────────────────────
  ctx.log.say('Fleet config — chassis + LLM endpoint (this box\'s private fleet network)')
  mkdirSync(join(state, 'fleet'), { recursive: true })
  const chassisPath = join(state, 'fleet/chassis.yml')
  if (o.force || !existsSync(chassisPath)) {
    const template = readFileSync(join(root, 'scripts/chassis.template.yml'), 'utf8')
    // chassis.yml is config, not a secret — plain 0644 write, like the template.
    writeFileSync(chassisPath, repointChassis(template, `devbox-${name}-fleet`))
    ctx.log.ok(`chassis.yml seeded (network: devbox-${name}-fleet)`)
  } else {
    ctx.log.ok('chassis.yml exists — kept (your edits survive; --force overwrites)')
  }
  const fleetEnvPath = join(state, 'fleet/.env')
  if (o.force || !existsSync(fleetEnvPath)) {
    const primary = existsSync(join(root, 'fleet/.env')) ? readFileSync(join(root, 'fleet/.env'), 'utf8') : ''
    writeSecret(fleetEnvPath, seedFleetEnv(primary))
    ctx.log.ok('fleet/.env seeded')
  } else {
    ctx.log.ok('fleet/.env exists — kept (your edits survive; --force overwrites)')
  }

  // NOTE for the future: gateway ports seeded from the same primary dump are
  // IDENTICAL across boxes by construction. That is fine — container-dial mode
  // (TALARIA_AGENT_DIAL=container, which every box sets) does not publish them.

  // ── Qdrant (optional) ──────────────────────────────────────────────────────
  if (o.qdrant) {
    ctx.log.say('Qdrant — snapshot round-trip (optional; the index is derived data)')
    // The devbox container is dual-homed (box network + primary dev network)
    // and carries curl: run the whole round-trip through it.
    const code = await ctx.run('docker', [
      'exec', `devbox-${name}`, 'sh', '-c',
      `set -e
for c in $(curl -sf http://${MAIN_QDRANT}:6333/collections | grep -o '"name":"[^"]*"' | cut -d'"' -f4); do
  snap=$(curl -sf -X POST http://${MAIN_QDRANT}:6333/collections/$c/snapshots | grep -o '"name":"[^"]*"' | head -1 | cut -d'"' -f4)
  curl -sf http://${MAIN_QDRANT}:6333/collections/$c/snapshots/$snap -o /tmp/$c.snapshot
  curl -sf -X POST -F "snapshot=@/tmp/$c.snapshot" http://devbox-${name}-qdrant:6333/collections/$c/snapshots/upload?priority=snapshot >/dev/null
  rm -f /tmp/$c.snapshot
  echo "  ok $c"
done`,
    ])
    if (code === 0) ctx.log.ok('qdrant restored')
    else ctx.log.warn('qdrant snapshot failed — re-run the KB backfill in the box\'s app instead')
  } else {
    ctx.log.warn('skipping Qdrant (derived index) — re-run the KB backfill in the box\'s app, or pass --qdrant')
  }
  return 0
}

export const seedCommand: Leaf = {
  kind: 'leaf',
  name: 'seed',
  summary: 'seed a box with starter data from the primary dev environment',
  usage: 'talaria box seed <name> [--force] [--qdrant]',
  positionals: { name: 'name', required: true },
  flags: [
    { name: 'force', kind: 'bool', desc: 're-copy over an existing seed (and chassis.yml / fleet/.env)' },
    { name: 'qdrant', kind: 'bool', desc: 'also round-trip the Qdrant index snapshot' },
  ],
  run: (ctx, args) =>
    runSeed(ctx, args.positionals[0]!, {
      force: args.flags.force === true,
      qdrant: args.flags.qdrant === true,
    }),
}
