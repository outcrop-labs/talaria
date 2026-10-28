// The dev stack's docker objects — the container names and network that
// docker/dev-compose.yml's `${VAR:-default}` spellings create — plus the
// inspect/ps probes three commands kept re-spelling. Kept here so the CLI and
// compose can never disagree about which container is "postgres", and so a
// value changed in one place is changed everywhere it is read.
//
// These are the compose file's DEFAULTS, which is to say the PRIMARY stack's.
// A command that acts on "this checkout's" postgres, redis or fleet resolves
// them through stack.ts (devStack), which names a worktree's own containers
// and refuses a shell override that names another stack's. Spelling
// `ctx.env.TALARIA_PG_CONTAINER ?? PG_CONTAINER` at the call site is how
// `talaria reset` run in a worktree came to reset main: nothing sets that var
// in a worktree. The primary's readiness probes in dev.ts and the seed
// SOURCES (worktree, box seed), which are main's by design, still read them
// directly.

import type { Ctx } from './ctx'

/** dev-compose.yml declares `name: talaria-dev`, so its containers land on
 *  this default bridge network — the one a box joins to dial the shared
 *  stateless service (embeddings). */
export const DEV_NETWORK = 'talaria-dev_default'

export const PG_CONTAINER = 'talaria-postgres-dev'
export const REDIS_CONTAINER = 'talaria-redis-dev'
export const QDRANT_CONTAINER = 'talaria-qdrant-dev'
export const EMBED_CONTAINER = 'talaria-embeddings-dev'
export const STORAGE_CONTAINER = 'talaria-storage-dev'

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

// No name-substring lister lives here any more. `talaria reset fleet` picked
// its containers by the substring `talaria-fleet-agent` and so, run in a
// worktree, removed main's agents. A stack's containers are selected by their
// compose project label (stack.ts, projectContainers).
