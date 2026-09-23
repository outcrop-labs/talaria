// The dev stack's docker objects — the container names and network that
// docker/dev-compose.yml's `${VAR:-default}` spellings create — plus the
// inspect/ps probes three commands kept re-spelling. Kept here so the CLI and
// compose can never disagree about which container is "postgres", and so a
// value changed in one place is changed everywhere it is read.
//
// These are the compose file's DEFAULTS. A reader that honours the operator's
// override spells compose's own `${VAR:-default}` at the call site —
// `ctx.env.TALARIA_PG_CONTAINER ?? PG_CONTAINER` — and a pure template that
// only needs the default uses the constant directly.

import type { Ctx } from './ctx'

/** dev-compose.yml declares `name: talaria-dev`, so its containers land on
 *  this default bridge network — the one a box joins to dial the shared
 *  stateless services (embeddings, SearXNG). */
export const DEV_NETWORK = 'talaria-dev_default'

export const PG_CONTAINER = 'talaria-postgres-dev'
export const REDIS_CONTAINER = 'talaria-redis-dev'
export const QDRANT_CONTAINER = 'talaria-qdrant-dev'
export const EMBED_CONTAINER = 'talaria-embeddings-dev'
export const MINIO_CONTAINER = 'talaria-minio-dev'
export const SEARCH_CONTAINER = 'talaria-searxng-dev'

/** A container's docker state as a string — 'running', 'exited', 'created',
 *  … — or null when the daemon does not know the name (never created, or the
 *  daemon is down). The `{{.State.Status}}` spelling: display code wants what
 *  docker calls it, not a bool. */
export async function containerState(ctx: Ctx, name: string): Promise<string | null> {
  try {
    return (await ctx.exec('docker', ['inspect', '-f', '{{.State.Status}}', name])).stdout.trim()
  } catch {
    return null
  }
}

/** Is the container's process RUNNING? Docker's own flag, deliberately: a
 *  PAUSED container is still running (its process is), and a probe that said
 *  otherwise would refuse to reset a stack that is merely frozen. False when
 *  the container does not exist — the same answer the old try/catch gave. */
export async function containerRunning(ctx: Ctx, name: string): Promise<boolean> {
  try {
    return (await ctx.exec('docker', ['inspect', '-f', '{{.State.Running}}', name])).stdout.trim() === 'true'
  } catch {
    return false
  }
}

/** Does the daemon know this container at all? Plain `docker inspect` with no
 *  format string: only existence is at stake, and a format would make the
 *  probe depend on a state the caller never reads. */
export async function containerExists(ctx: Ctx, name: string): Promise<boolean> {
  try {
    await ctx.exec('docker', ['inspect', name])
    return true
  } catch {
    return false
  }
}

/** Names of the RUNNING containers whose name contains `filter` — or, with
 *  `all`, every container docker knows. Docker refusing to list (daemon down)
 *  is an empty answer, not an error: every caller is reporting, not acting. */
export async function containerNames(ctx: Ctx, filter: string, all = false): Promise<string[]> {
  const args = ['ps', ...(all ? ['-a'] : []), '--format', '{{.Names}}']
  try {
    return (await ctx.exec('docker', args)).stdout.split('\n').filter((l) => l.includes(filter))
  } catch {
    return []
  }
}