// docker compose plumbing — the tuple builders and waits the bash scripts
// kept re-spelling. The composeArgs shape mirrors the Rust fleet docker
// builder's (api/src/fleet/docker.rs): every caller goes through ONE place,
// so the -p/-f/--env-file convention cannot drift between commands.

import type { Ctx } from './ctx'

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
