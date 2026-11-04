// Seeding a new checkout's Rust build cache from an idle one, by reflink.
//
// WHY. Every worktree builds into its own `api/target` (one shared target dir
// would serialise every agent behind cargo's build-dir lock), so every new
// worktree paid a cold build and wrote its own copy of the same third-party
// artifacts: four agent worktrees held 15-107 GiB each on 2026-11-03. sccache
// saves the CPU of that, not the disk, because a cache hit still writes the
// full artifact into the new target.
//
// WHAT IS COPIED. Only the units of packages the new checkout takes from a
// registry or git source, and only by reflink (`cp --reflink=always`), so the
// copy shares the source's extents until either side rewrites a file.
// Nothing of a PATH package (a workspace crate) is copied: not its
// fingerprints, not its rlibs or test binaries, not `incremental/`, not the
// uplifted files in `debug/`. Every workspace crate is compiled from this
// checkout's own sources on the first build, with no artifact of another
// tree to mistake for its own.
//
// WHY THAT AND NOT A WHOLE COPY. A copied target reused STALE workspace
// artifacts (2026-10-23, `unresolved import` for a symbol that existed). Two
// facts make that happen:
//   - a path package's unit hash (`-C metadata`, the `-<hash>` in every file
//     name) is computed from its path RELATIVE to the workspace root, so
//     crate `x` in worktree A and crate `x` in worktree B, with different
//     sources, look for the same artifact names;
//   - a path package's freshness is an mtime test: fresh while no source file
//     listed in its dep-info is newer than the dep-info file. `cp -r` gives
//     every copied file the copy's own time, later than the new checkout's
//     sources, so every workspace crate read as fresh. Even `cp -a`, which
//     keeps the mtimes, is only right while no build in the source started
//     after the new checkout was written.
// A registry unit has neither problem: its fingerprint is its version (the
// source under ~/.cargo is immutable), its features and flags, so cargo
// judges it exactly as it would in the tree it came from.
//
// SAFETY. A target another cargo is building in is never read: its lock
// files are checked in /proc/locks first, and the copy itself runs under
// SHARED flocks on them (cargo 1.97 takes `.cargo-build-lock` and
// `.cargo-artifact-lock` exclusively, and `.cargo-lock` shared), so a build
// that starts during the copy waits for it. No reflink, no seed: a
// filesystem without it gets a cold build, never a hard link (which would
// share mutable files between trees) and never a full byte copy. Any failure
// removes the half-made target and leaves the cold build.

import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, statfsSync, statSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import type { Ctx } from './ctx'

/** The two cargo roots this repo builds, each with its own lockfile and
 *  target dir. Relative to a checkout root. */
export const CARGO_ROOTS = { api: 'api', desktop: 'desktop/src-tauri' } as const
export type CargoRoot = keyof typeof CARGO_ROOTS

/** The profile dir seeded. `dev` and `test` both build into it, and so do
 *  `check` and clippy; a release build is rare enough in a worktree to pay
 *  for itself. */
export const SEED_PROFILE = 'debug'

/** The lock files cargo takes on a profile dir. 1.97 takes `.cargo-lock`
 *  shared (for older cargos, which took it exclusively) and the other two
 *  exclusively. */
export const CARGO_LOCK_FILES = ['.cargo-lock', '.cargo-build-lock', '.cargo-artifact-lock'] as const

/** Written into a seeded target, so the sweep and a human can tell. */
export const SEED_MARKER = '.talaria-seed.json'

export type LockPackages = {
  /** Names with at least one registry or git entry. */
  registry: Set<string>
  /** Names with at least one entry that has no `source` — a path package. */
  path: Set<string>
  /** `name version source` of every registry or git entry. */
  sourced: Set<string>
}

/** The packages a Cargo.lock names, split by where they come from. Pure. */
export function lockPackages(lock: string): LockPackages {
  const out: LockPackages = { registry: new Set(), path: new Set(), sourced: new Set() }
  const flush = (pkg: { name?: string; version?: string; source?: string }) => {
    if (!pkg.name) return
    if (pkg.source) {
      out.registry.add(pkg.name)
      out.sourced.add(`${pkg.name} ${pkg.version ?? ''} ${pkg.source}`)
    } else {
      out.path.add(pkg.name)
    }
  }
  let cur: { name?: string; version?: string; source?: string } | null = null
  for (const line of lock.split('\n')) {
    const t = line.trim()
    if (t.startsWith('[')) {
      if (cur) flush(cur)
      cur = t === '[[package]]' ? {} : null
      continue
    }
    if (!cur) continue
    const m = /^(name|version|source)\s*=\s*"([^"]*)"$/.exec(t)
    if (m) cur[m[1] as 'name' | 'version' | 'source'] = m[2]
  }
  if (cur) flush(cur)
  return out
}

/** `<package>-<16 hex>`, how cargo names a unit's fingerprint and build dirs. */
const UNIT_DIR_RE = /^(.+)-([0-9a-f]{16})$/
/** The unit hash in a `deps/` file name: `libfoo-<hash>.rlib`, `foo-<hash>`. */
const DEPS_HASH_RE = /-([0-9a-f]{16})(?:\.|$)/

export type SeedSelection = {
  /** Paths relative to the profile dir, to copy. */
  copy: string[]
  /** Registry or git units kept. */
  units: number
  /** Path-package units left out (they build from this checkout's sources). */
  pathUnits: number
}

/**
 * What to copy out of a profile dir, given its three listings and the NEW
 * checkout's lockfile. Pure.
 *
 * A unit is kept only when its package name is one the new lockfile takes
 * from a registry or git source, and is NOT also a path package there (a
 * name that is both is left out: rebuilding a registry crate costs time,
 * reusing a workspace crate costs correctness). A `deps/` file is copied
 * only when its hash is a kept unit's, so a file no kept fingerprint vouches
 * for never arrives. Nothing else in the profile dir is copied: not
 * `incremental/` (workspace crates only), not `examples/`, not the uplifted
 * binaries and rlibs at its top level.
 */
export function seedSelection(
  listing: { fingerprint: string[]; build: string[]; deps: string[] },
  pkgs: LockPackages,
): SeedSelection {
  const keepName = (name: string) => pkgs.registry.has(name) && !pkgs.path.has(name)
  const hashes = new Set<string>()
  const copy: string[] = []
  let units = 0
  let pathUnits = 0
  for (const d of listing.fingerprint) {
    const m = UNIT_DIR_RE.exec(d)
    if (!m) continue
    if (keepName(m[1]!)) {
      hashes.add(m[2]!)
      copy.push(`.fingerprint/${d}`)
      units++
    } else if (pkgs.path.has(m[1]!)) {
      pathUnits++
    }
  }
  for (const d of listing.build) {
    const m = UNIT_DIR_RE.exec(d)
    if (m && keepName(m[1]!) && hashes.has(m[2]!)) copy.push(`build/${d}`)
  }
  for (const f of listing.deps) {
    const m = DEPS_HASH_RE.exec(f)
    if (m && hashes.has(m[1]!)) copy.push(`deps/${f}`)
  }
  return { copy, units, pathUnits }
}

/** A device number as /proc/locks spells it (`fc:00`): hex major:minor. */
export function procLocksDev(dev: number): string {
  const major = Math.floor(dev / 256) & 0xfff
  const minor = (dev & 0xff) | ((Math.floor(dev / 4096) >>> 0) & ~0xff & 0xfffff)
  return `${major.toString(16).padStart(2, '0')}:${minor.toString(16).padStart(2, '0')}`
}

/** The `dev:inode` keys of every flock /proc/locks lists. Pure. */
export function heldFlocks(procLocks: string): Set<string> {
  const out = new Set<string>()
  for (const line of procLocks.split('\n')) {
    // `1: FLOCK  ADVISORY  WRITE 3512 fc:00:621256963 0 EOF`
    const m = /^\d+:\s+FLOCK\s+\S+\s+\S+\s+\d+\s+([0-9a-f]+:[0-9a-f]+):(\d+)\s/.exec(line.replace(/^\s*\d+:\s+->\s+/, '0: '))
    if (m) out.add(`${m[1]}:${m[2]}`)
  }
  return out
}

/**
 * Is a cargo building in this profile dir? true, false, or null when it
 * cannot be read (no /proc/locks: not Linux). Read-only: it takes no lock,
 * so it can look at another session's target without slowing its build.
 */
export function profileBusy(profileDir: string, procLocks: string | null = readProcLocks()): boolean | null {
  if (procLocks === null) return null
  // btrfs: stat() answers the subvolume's anonymous device and /proc/locks the
  // superblock's, so nothing would ever match. Unknown, then: the shared
  // flock around the copy is the real gate either way.
  try {
    if (Number(statfsSync(profileDir).type) === BTRFS_SUPER_MAGIC) return null
  } catch {
    return null
  }
  const held = heldFlocks(procLocks)
  for (const f of CARGO_LOCK_FILES) {
    try {
      const st = statSync(join(profileDir, f), { bigint: true })
      if (held.has(`${procLocksDev(Number(st.dev))}:${st.ino}`)) return true
    } catch {
      // that lock file does not exist: nothing can hold it
    }
  }
  return false
}

/** statfs f_type of btrfs. */
export const BTRFS_SUPER_MAGIC = 0x9123683e

export function readProcLocks(): string | null {
  try {
    return readFileSync('/proc/locks', 'utf8')
  } catch {
    return null
  }
}

/** When a target was last built in: its profile dir and the two dirs every
 *  build writes into. A target's own mtime moves only when a profile dir is
 *  created, so it is no clock of use. */
export function targetLastUsedMs(targetDir: string): number {
  let max = 0
  for (const p of [targetDir, join(targetDir, SEED_PROFILE), join(targetDir, SEED_PROFILE, 'deps'), join(targetDir, SEED_PROFILE, '.fingerprint')]) {
    try {
      max = Math.max(max, statSync(p).mtimeMs)
    } catch {
      // absent: says nothing
    }
  }
  return max
}

export type SeedCandidate = { checkout: string; overlap: number; lastUsedMs: number }

/** Best source first: the most `name version source` entries shared with
 *  the new lockfile (each one a unit that can be reused rather than built),
 *  then the most recently built. Pure. */
export function rankSources(cands: SeedCandidate[]): SeedCandidate[] {
  return [...cands].sort((a, b) => b.overlap - a.overlap || b.lastUsedMs - a.lastUsedMs)
}

export type SeedResult =
  | { seeded: true; from: string; units: number; pathUnits: number; files: number; ms: number }
  /** `failed`: a seed was possible and did not happen (busy source, no
   *  reflink, a copy that broke) — worth a warning. Otherwise there was
   *  nothing to seed from, which is a fresh machine's normal. */
  | { seeded: false; why: string; failed?: true }

/** Every other checkout git lists, with a `<rel>/target/debug` to read. */
export async function seedCandidates(ctx: Ctx, toCheckout: string, root: CargoRoot): Promise<SeedCandidate[]> {
  const rel = CARGO_ROOTS[root]
  let listed = ''
  try {
    listed = (await ctx.exec('git', ['worktree', 'list', '--porcelain'], { cwd: toCheckout })).stdout
  } catch {
    return []
  }
  const want = lockText(toCheckout, root)
  const wanted = want ? lockPackages(want).sourced : new Set<string>()
  const out: SeedCandidate[] = []
  for (const line of listed.split('\n')) {
    if (!line.startsWith('worktree ')) continue
    const checkout = line.slice('worktree '.length)
    if (checkout === toCheckout) continue
    const target = join(checkout, rel, 'target')
    if (!existsSync(join(target, SEED_PROFILE, '.fingerprint'))) continue
    const theirs = lockText(checkout, root)
    let overlap = 0
    if (theirs) for (const s of lockPackages(theirs).sourced) if (wanted.has(s)) overlap++
    out.push({ checkout, overlap, lastUsedMs: targetLastUsedMs(target) })
  }
  return rankSources(out)
}

function lockText(checkout: string, root: CargoRoot): string | null {
  try {
    return readFileSync(join(checkout, CARGO_ROOTS[root], 'Cargo.lock'), 'utf8')
  } catch {
    return null
  }
}

/**
 * Seed `<toCheckout>/<root>/target` from `fromCheckout`'s, or say why not.
 * Never throws for a seed that cannot happen: the answer is a cold build.
 * Refuses a target that already exists (it is someone's cache).
 */
export async function seedTarget(ctx: Ctx, fromCheckout: string, toCheckout: string, root: CargoRoot): Promise<SeedResult> {
  const rel = CARGO_ROOTS[root]
  const srcProfile = join(fromCheckout, rel, 'target', SEED_PROFILE)
  const dstTarget = join(toCheckout, rel, 'target')
  const dstProfile = join(dstTarget, SEED_PROFILE)
  if (fromCheckout === toCheckout) return { seeded: false, why: 'the source is this checkout' }
  if (existsSync(dstTarget)) return { seeded: false, why: `${dstTarget} already exists (kept)` }
  if (!existsSync(join(srcProfile, '.fingerprint'))) return { seeded: false, why: `${srcProfile} has no build to seed from` }
  const lock = lockText(toCheckout, root)
  if (!lock) return { seeded: false, why: `no ${join(rel, 'Cargo.lock')} in ${toCheckout}` }
  if (profileBusy(srcProfile) === true) return { seeded: false, why: `a cargo is building in ${srcProfile}`, failed: true }
  try {
    await ctx.exec('flock', ['--version'])
  } catch {
    return { seeded: false, why: 'no `flock` on PATH (util-linux): the source cannot be locked while it is read', failed: true }
  }

  let listing: { fingerprint: string[]; build: string[]; deps: string[] }
  try {
    const ls = (d: string) => (existsSync(join(srcProfile, d)) ? readdirSync(join(srcProfile, d)) : [])
    listing = { fingerprint: ls('.fingerprint'), build: ls('build'), deps: ls('deps') }
  } catch (e) {
    return { seeded: false, why: `could not list ${srcProfile}: ${e instanceof Error ? e.message : String(e)}` }
  }
  const sel = seedSelection(listing, lockPackages(lock))
  const probe = sel.copy.find((p) => p.startsWith('deps/'))
  if (!probe) return { seeded: false, why: `${srcProfile} holds no unit this checkout's lockfile can use` }

  const started = Date.now()
  const undo = (why: string): SeedResult => {
    rmSync(dstTarget, { recursive: true, force: true })
    return { seeded: false, why, failed: true }
  }
  // Nothing past here may throw: a half-made target would block every later
  // seed as "already exists", and fail the worktree whose stack is already up.
  try {
    return await copyUnder(ctx, { srcProfile, dstTarget, dstProfile, fromCheckout, rel, sel, probe, started, undo })
  } catch (e) {
    return undo(`the seed failed: ${String(e instanceof Error ? e.message : e).split('\n')[0]}`)
  }
}

async function copyUnder(
  ctx: Ctx,
  o: {
    srcProfile: string
    dstTarget: string
    dstProfile: string
    fromCheckout: string
    rel: string
    sel: SeedSelection
    probe: string
    started: number
    undo: (why: string) => SeedResult
  },
): Promise<SeedResult> {
  const { srcProfile, dstTarget, dstProfile, fromCheckout, rel, sel, probe, started, undo } = o
  mkdirSync(dstProfile, { recursive: true })
  // Reflink or nothing. `--reflink=always` fails where the filesystem cannot
  // share extents (ext4, tmpfs, across filesystems) instead of copying bytes.
  try {
    await ctx.exec('cp', ['--reflink=always', join(srcProfile, probe), join(dstTarget, '.reflink-probe')])
    rmSync(join(dstTarget, '.reflink-probe'), { force: true })
  } catch {
    return undo(`this filesystem cannot reflink ${srcProfile} into ${dstTarget}: a cold build, rather than a full copy or a hard link`)
  }
  const list = join(dstTarget, '.seed-list')
  writeFileSync(list, sel.copy.join('\0'))
  // Shared flocks on every lock file the source has, held for the whole
  // copy: a cargo already building there fails `-n` (the target is skipped);
  // one that starts during the copy waits for it.
  const locks = CARGO_LOCK_FILES.map((f) => join(srcProfile, f)).filter((f) => existsSync(f))
  const argv: string[] = []
  for (const l of locks) argv.push('flock', '--shared', '--nonblock', '--conflict-exit-code', '75', l)
  argv.push('xargs', '-0', '-a', list, 'cp', '-a', '--reflink=always', '--parents', '-t', dstProfile)
  try {
    await ctx.exec(argv[0]!, argv.slice(1), { cwd: srcProfile, timeoutMs: 30 * 60_000 })
  } catch (e) {
    const busy = (e as { code?: unknown })?.code === 75
    return undo(busy ? `a cargo started building in ${srcProfile}` : `the copy failed: ${String(e instanceof Error ? e.message : e).split('\n')[0]}`)
  }
  rmSync(list, { force: true })
  writeFileSync(
    join(dstTarget, SEED_MARKER),
    JSON.stringify({ from: join(fromCheckout, rel, 'target'), at: new Date(ctx.now()).toISOString(), units: sel.units }, null, 2) + '\n',
  )
  return { seeded: true, from: fromCheckout, units: sel.units, pathUnits: sel.pathUnits, files: sel.copy.length, ms: Date.now() - started }
}

/** Seed from the best idle candidate, or from `from` when one is named.
 *  Tries one source: a busy or failed one is a cold build, not a hunt. */
export async function seedFromBest(ctx: Ctx, toCheckout: string, root: CargoRoot, from?: string): Promise<SeedResult> {
  if (from) return seedTarget(ctx, from, toCheckout, root)
  const cands = (await seedCandidates(ctx, toCheckout, root)).filter(
    (c) => profileBusy(join(c.checkout, CARGO_ROOTS[root], 'target', SEED_PROFILE)) !== true,
  )
  if (!cands.length) return { seeded: false, why: `no idle checkout has a ${CARGO_ROOTS[root]}/target to seed from` }
  return seedTarget(ctx, cands[0]!.checkout, toCheckout, root)
}

/** One line for the log. */
export function describeSeed(root: CargoRoot, r: SeedResult): string {
  const t = `${CARGO_ROOTS[root]}/target`
  return r.seeded
    ? `${t} seeded from ${r.from} by reflink: ${r.units} registry units reused, ${r.pathUnits} workspace units left to build here (${(r.ms / 1000).toFixed(1)} s)`
    : `${t} not seeded — ${r.why}; the first build is cold`
}

export type CheckoutSeedOpts = { from?: string; desktop?: boolean }

/** Seed a checkout's api target, and its desktop target with `desktop`,
 *  logging one line each. A seed that cannot happen is a warning, never a
 *  failure: the build it was meant to shorten still works cold. */
export async function seedCheckout(ctx: Ctx, checkout: string, opts: CheckoutSeedOpts = {}): Promise<SeedResult[]> {
  const out: SeedResult[] = []
  const roots: CargoRoot[] = opts.desktop ? ['api', 'desktop'] : ['api']
  for (const root of roots) {
    const r = await seedFromBest(ctx, checkout, root, opts.from)
    if (r.seeded) ctx.log.ok(describeSeed(root, r))
    else if (r.failed) ctx.log.warn(describeSeed(root, r))
    else ctx.log.skip(describeSeed(root, r))
    out.push(r)
  }
  return out
}
