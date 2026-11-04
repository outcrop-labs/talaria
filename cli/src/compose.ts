// docker compose plumbing — the tuple builders and waits the bash scripts
// kept re-spelling. The composeArgs shape mirrors the Rust fleet docker
// builder's (api/src/fleet/docker.rs): every caller goes through ONE place,
// so the -p/-f/--env-file convention cannot drift between commands.
//
// It is also the home of the COMPOSE_FILE law (composeFileEnv +
// composeFileArgs): an explicit `-f` BEATS the env in docker's own
// precedence, so a caller that honors the operator's layering drops its
// `-f` — and the env, whose list replaces both files, is repaired to carry
// the sidecar plane — spelled once, here.

import { join } from 'node:path'
import type { Ctx } from './ctx'
import { profileFiles, repairComposeFileExport, tildePath } from './profile-export'

/** The canonical production compose file — the RELATIVE path CONTAINER.md
 *  tells operators to type, which is why every printed equivalent shows it
 *  spelled exactly this way. */
export const COMPOSE_BASE = 'docker/compose.yml'

/** The SHARED sidecar plane — postgres, redis, qdrant, embeddings, storage
 *  with their images, restart policies, data volumes, env defaults and
 *  healthchecks — which EVERY stack layers IN FRONT of its own file:
 *
 *    docker compose -f docker/sidecars.compose.yml -f docker/dev-compose.yml …
 *
 *  FIRST, always. Compose merges by service name with the later file winning,
 *  so a stack's own container_name / ports / networks / divergent env only
 *  override if they come after it — and docker/devbox.compose.yml's `!reset`
 *  of the two services a box must not run only sticks for the same reason
 *  (a fragment listed last would hand the box a second TEI).
 *  It is deliberately NOT part of the COMPOSE_FILE env flow: that env names
 *  the operator's own layering, and this file is not their override. */
export const SIDECARS_COMPOSE = 'docker/sidecars.compose.yml'

/** The third file of a `talaria worktree` stack: its postgres and redis on
 *  the default bridge, so the project creates no network of its own and
 *  takes no subnet from the daemon's address pools (the file says why). Every
 *  compose call that brings a worktree's services up names it — a call that
 *  left it out would see a different config and recreate them on a network. */
export const WORKTREE_COMPOSE = 'docker/worktree.compose.yml'

/** A worktree stack's files: the sidecar plane, the dev loop's file, then
 *  WORKTREE_COMPOSE. */
export function worktreeComposeFiles(root: string): string[] {
  return stackComposeFiles(root, 'docker/dev-compose.yml', WORKTREE_COMPOSE)
}

/** A stack's compose file list: the shared sidecar plane FIRST, then the
 *  stack's own file(s) — the ONE place the merge order is decided, so no
 *  caller can spell it the other way round (see SIDECARS_COMPOSE for why
 *  first is load-bearing). `stack` entries are repo-relative, as the CLI's
 *  other compose-file constants are. */
export function stackComposeFiles(root: string, ...stack: string[]): string[] {
  return [join(root, SIDECARS_COMPOSE), ...stack.map((f) => join(root, f))]
}

/** An operator-provided COMPOSE_FILE (the registry-image flow in
 *  CONTAINER.md), trimmed and REPAIRED: the sidecar plane is prepended when
 *  the export omits it, with a warning, and the corrected list is written
 *  back into ctx.env — the docker child reads COMPOSE_FILE from the
 *  inherited environment, not from this string, so the repair only reaches
 *  compose by re-exporting it. The write-back is also the once-only gate:
 *  every later call reads the repaired value and passes it through. Null
 *  when unset or whitespace — the canonical single-file path.
 *
 *  The export itself is repaired too, where it lives (profile-export.ts):
 *  the in-memory repair alone left the same warning on every update for as
 *  long as the profile line stayed wrong. Fixed there, the run says so once;
 *  a shell that still holds the old value gets one quiet line; only an export
 *  the CLI cannot find or cannot write keeps the warning. */
export function composeFileEnv(ctx: Ctx): string | null {
  const value = ctx.env.COMPOSE_FILE?.trim()
  if (!value) return null
  const repaired = withSidecarPlane(value)
  if (repaired !== value) {
    ctx.env.COMPOSE_FILE = repaired
    const home = ctx.env.HOME
    const results = repairComposeFileExport(profileFiles(ctx.env), value, repaired)
    const named = (status: string) => results.filter((r) => r.status === status).map((r) => tildePath(r.file, home))
    const fixed = named('fixed')
    const readonly = named('readonly')
    const already = named('already')
    if (fixed.length > 0 && readonly.length === 0) {
      ctx.log.ok(
        `fixed the COMPOSE_FILE export in ${fixed.join(', ')}: ${SIDECARS_COMPOSE} now leads the list. ` +
          'New shells read the fixed line; this run already uses it.',
      )
    } else if (already.length > 0 && readonly.length === 0) {
      ctx.log.skip(
        `this shell's COMPOSE_FILE predates the fix in ${already.join(', ')} — a new shell (or \`source ${already[0]}\`) ends this line`,
      )
    } else {
      const there = readonly.length > 0 ? ` It is exported in ${readonly.join(', ')}, which this user cannot write — fix it there.` : ''
      ctx.log.warn(
        `COMPOSE_FILE=${value} omits ${SIDECARS_COMPOSE} — prepending it. The env replaces the wrappers' -f pair ` +
          `and the sidecar plane's images live only in that fragment; spell it first in the export to keep this quiet.${there}`,
      )
    }
  }
  return repaired
}

/** The `-f` args for a compose invocation of `base`, under the one law every
 *  caller shares: docker's precedence puts an explicit `-f` ABOVE the
 *  COMPOSE_FILE env, so honoring the operator's layering means dropping the
 *  `-f` entirely and letting the env name the files. The argument is the
 *  already-resolved COMPOSE_FILE value (composeFileEnv(ctx) at a ctx-bearing
 *  call site; unit.ts renders the unit from an opts field, with no Ctx).
 *
 *  `base` is ALWAYS preceded by the shared sidecar plane: the sidecars are not
 *  an operator override, they are half the project, and without them the
 *  stack refuses to start (`service "postgres" has neither an image nor a
 *  build context`). Under COMPOSE_FILE the operator names the files, so THAT
 *  list has to carry the fragment itself — composeFileEnv guarantees it does
 *  (prepended with a warning when the export forgot; two customer VMs,
 *  2026-09-21). */
export function composeFileArgs(composeFile: string | null | undefined, base: string): string[] {
  return composeFile ? [] : ['-f', SIDECARS_COMPOSE, '-f', base]
}

/** The export with the shared sidecar plane guaranteed IN it, first. The
 *  plane is half the project, not an operator override — its six services'
 *  images are defined nowhere else — so a list without it is an invalid
 *  project, and PREPEND is the repair that preserves the operator's own
 *  layering exactly (compose merges later-wins, which is also why the plane
 *  must lead). A list that already carries the fragment — anywhere — passes
 *  through byte-identical. The die this replaces spelled the same fix out,
 *  but left every install that followed the pre-fragment docs unable to
 *  update or deploy until hands re-exported on each host. */
export function withSidecarPlane(composeFile: string): string {
  const listed = composeFile.split(':').map((p) => p.trim()).filter(Boolean)
  return listed.includes(SIDECARS_COMPOSE) ? composeFile : [SIDECARS_COMPOSE, ...listed].join(':')
}

export type ComposeSpec = {
  /** Compose files in merge order (later wins). */
  files: string[]
  project?: string
  /** Interpolation env file. */
  envFile?: string
}

/** The literal argv a `docker compose` invocation is made of. */
export function composeArgs(spec: ComposeSpec): string[] {
  const args: string[] = []
  for (const f of spec.files) args.push('-f', f)
  if (spec.project) args.push('-p', spec.project)
  if (spec.envFile) args.push('--env-file', spec.envFile)
  return args
}

/** Run a compose operation, inheriting the terminal. A failed `up` is asked
 *  one more question — is docker out of network address pools? — because
 *  compose's own line for that says nothing about what to do. */
export async function compose(ctx: Ctx, spec: ComposeSpec, op: string[]): Promise<number> {
  const code = await ctx.run('docker', ['compose', ...composeArgs(spec), ...op])
  if (code !== 0 && op[0] === 'up') await explainPoolExhaustion(ctx)
  return code
}

/** docker's words when no predefined address pool has a subnet left: the
 *  daemon's own (`network create`, compose) and the older libnetwork one. */
export const POOL_EXHAUSTED_RE =
  /all predefined address pools have been fully subnetted|could not find an available,? non-overlapping ipv[46] address pool/i

/** The probe network's name. It carries the `talaria.pool-probe` label and
 *  no compose project, and the cleanup sweep removes one left behind. */
export const POOL_PROBE_NETWORK = `talaria-pool-probe-${process.pid}`

/** After a failed `up`: could docker create a network at all right now? The
 *  answer cannot come from compose's output — the terminal owns it (stdio is
 *  inherited, so its progress UI stays intact) — so ask docker directly with a
 *  throwaway network, removed at once when it is made. Only the pool answer
 *  says anything; every other failure is compose's own to report.
 *
 *  The incident (follow-up A8, 2026-10-27): every worktree, devbox and app
 *  database project creates its own network, the daemon's default pools hold
 *  about thirty, and with parallel stacks `up` failed with "all predefined
 *  address pools have been fully subnetted". Networks a stopped or
 *  interrupted stack left behind hold pools too; `talaria cleanup` lists them
 *  and `--apply` removes them. */
export async function explainPoolExhaustion(ctx: Ctx): Promise<boolean> {
  try {
    await ctx.exec('docker', ['network', 'create', '--label', 'talaria.pool-probe=1', POOL_PROBE_NETWORK])
  } catch (e) {
    const why = e instanceof Error ? e.message : String(e)
    if (!POOL_EXHAUSTED_RE.test(why)) return false
    ctx.log.warn(
      'docker has no network address space left: every predefined address pool is fully subnetted, so no ' +
        'compose project can create the network it needs. Networks that stopped or interrupted stacks left ' +
        'behind still hold their subnets — `bun talaria cleanup` lists the ones no worktree, box or running ' +
        'stack owns, and `bun talaria cleanup --apply` removes them. `docker network ls` shows every network; ' +
        'a host that runs many stacks at once can also give the daemon more room (`default-address-pools` in ' +
        '/etc/docker/daemon.json).',
    )
    return true
  }
  try {
    await ctx.exec('docker', ['network', 'rm', POOL_PROBE_NETWORK])
  } catch {
    // A probe network that would not go is one small network, labelled
    // talaria.pool-probe; it does not change the answer.
  }
  return false
}

/** Poll until `probe` resolves true, `attempts` × interval. Returns the last
 *  answer — the postgres / redis-cli waits, without the bash sleep loop. */
export async function waitFor(
  ctx: Ctx,
  what: string,
  probe: () => Promise<boolean>,
  attempts: number,
  intervalMs = 1000,
): Promise<boolean> {
  let ok = false
  for (let i = 0; i < attempts; i++) {
    ok = await probe()
    if (ok) return true
    await ctx.sleep(intervalMs)
  }
  return ok
}

/** The readiness probe for a stack's Postgres, as `docker exec` argv: a real
 *  query against the `talaria` database, over TCP.
 *
 *  `pg_isready` over the socket was the probe, and it lies twice on a fresh
 *  volume. The image's entrypoint first runs a TEMPORARY server with
 *  `listen_addresses=''` — socket only — to create POSTGRES_DB and run the
 *  init scripts, then shuts it down and starts the real one on TCP. The
 *  socket probe passes against that temp server, before the database exists
 *  and moments before its fast shutdown; and `pg_isready -d` answers
 *  "accepting connections" for a database that does not exist at all. So a
 *  worktree's seed, run straight after the wait, died with `FATAL: the
 *  database system is shutting down` or `database "talaria" does not exist`
 *  (follow-up A1, 2026-10-27 — five of the run's first worktrees). The temp
 *  server has no TCP listener, and before init there is no database to
 *  query, so this probe cannot pass until the real server serves the real
 *  database. Loopback TCP inside the container is `trust` in the image's
 *  pg_hba.conf, so no password rides along. */
export function pgReadyArgs(container: string): string[] {
  return ['exec', container, 'psql', '-h', '127.0.0.1', '-U', 'talaria', '-d', 'talaria', '-tAc', 'select 1']
}

/** Wait for `container`'s Postgres to serve the `talaria` database
 *  (pgReadyArgs says why it is a query over TCP). The answer is the caller's
 *  to act on — every caller dies on false. */
export function waitForPostgres(ctx: Ctx, container: string, attempts: number): Promise<boolean> {
  return waitFor(
    ctx,
    `postgres (${container})`,
    async () => {
      try {
        await ctx.exec('docker', pgReadyArgs(container))
        return true
      } catch {
        return false
      }
    },
    attempts,
  )
}

/** The `docker network inspect x || docker network create x` idiom,
 *  race-safe the way the Rust fleet builder's is (api/src/fleet/docker.rs):
 *  re-inspect after a failed create, because "already exists" from a
 *  concurrent creator is success. */
export async function ensureNetwork(ctx: Ctx, name: string): Promise<'exists' | 'created'> {
  const has = async () => {
    try {
      await ctx.exec('docker', ['network', 'inspect', name])
      return true
    } catch {
      return false
    }
  }
  if (await has()) return 'exists'
  try {
    await ctx.exec('docker', ['network', 'create', name])
    return 'created'
  } catch {
    if (await has()) return 'exists'
    throw new Error(`docker network ${name} could not be created`)
  }
}
