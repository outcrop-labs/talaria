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

/** The canonical production compose file — the RELATIVE path CONTAINER.md
 *  tells operators to type, which is why every printed equivalent shows it
 *  spelled exactly this way. */
export const COMPOSE_BASE = 'docker/compose.yml'

/** The SHARED sidecar plane — postgres, redis, qdrant, embeddings, minio,
 *  searxng with their images, restart policies, data volumes, env defaults and
 *  healthchecks — which EVERY stack layers IN FRONT of its own file:
 *
 *    docker compose -f docker/sidecars.compose.yml -f docker/dev-compose.yml …
 *
 *  FIRST, always. Compose merges by service name with the later file winning,
 *  so a stack's own container_name / ports / networks / divergent env only
 *  override if they come after it — and docker/devbox.compose.yml's `!reset`
 *  of the two services a box must not run only sticks for the same reason
 *  (a fragment listed last would hand the box a second TEI + SearXNG).
 *  It is deliberately NOT part of the COMPOSE_FILE env flow: that env names
 *  the operator's own layering, and this file is not their override. */
export const SIDECARS_COMPOSE = 'docker/sidecars.compose.yml'

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
 *  when unset or whitespace — the canonical single-file path. */
export function composeFileEnv(ctx: Ctx): string | null {
  const value = ctx.env.COMPOSE_FILE?.trim()
  if (!value) return null
  const repaired = withSidecarPlane(value)
  if (repaired !== value) {
    ctx.log.warn(
      `COMPOSE_FILE=${value} omits ${SIDECARS_COMPOSE} — prepending it. The env replaces the wrappers' -f pair ` +
        'and the sidecar plane\'s images live only in that fragment; spell it first in the export to keep this quiet.',
    )
    ctx.env.COMPOSE_FILE = repaired
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

/** Run a compose operation, inheriting the terminal. */
export function compose(ctx: Ctx, spec: ComposeSpec, op: string[]): Promise<number> {
  return ctx.run('docker', ['compose', ...composeArgs(spec), ...op])
}

/** Poll until `probe` resolves true, `attempts` × interval. Returns the last
 *  answer — the pg_isready / redis-cli waits, without the bash sleep loop. */
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
    await new Promise((r) => setTimeout(r, intervalMs))
  }
  return ok
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
