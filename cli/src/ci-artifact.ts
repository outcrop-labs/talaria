// The talaria-api binary CI built, fetched and run instead of `cargo run`:
// `talaria dev --api-from-ci`.
//
// WHY. Compiling the api on this machine pins it and locks `api/target` for
// every other agent here, and a worktree needed a 30 GiB target to do it. CI
// already builds the binary: ci.yml's api job uploads it as the artifact
// `talaria-api-linux-x64-<sha>` (a stripped dev build, kept three days) for a
// same-repository pull request, a push to `testing`, and an offload run
// (`bun run gate:remote`) whose plan has talaria-api in scope — scripts/
// surfaces.mjs, binQualifies. Exercising a change then needs a download, not
// a build.
//
// WHICH BINARY. One built from exactly this tree's api inputs (api/ and the
// files outside it the binary compiles in — API_BINARY_INPUTS): HEAD's own,
// then its first parents' for as long as their api inputs are still HEAD's
// (at most WALK_LIMIT commits). That walk is the branch's own commits (an
// offload run before a docs-only commit), then the merge-base with the
// branch it was cut from, then that branch's history: a testing push builds
// the binary only when it moved the api, so the merge-base itself rarely has
// one, while the last testing commit that did usually does — the same build.
// A branch that changed the api stops at its last api commit, and the base's
// binary, the code before the change, is never taken.
// Uncommitted api changes are refused: no CI binary contains them.
//
// A pull request's binary is built from the pull request's merge ref
// (actions/checkout's default for that event), so it carries the base's api
// as of that run as well as the branch's: what the merge would serve, and
// still named for the head commit, the sha `workflow_run.head_sha` gives.
//
// TRUST. The file is executed on this machine, so an artifact counts only
// when a run of THIS repository made it for exactly the commit asked for. A
// fork's pull request runs ci.yml from the fork's branch, in this repository,
// and could upload any file under any name — a public testing sha included:
// `workflow_run.head_repository_id` must equal `repository_id`, and
// `workflow_run.head_sha` must be the commit. The zip's sha256 must equal the
// artifact's `digest`, and the entry's CRC its own.
//
// WHERE. `api/.ci-bin/<sha12>/talaria-api`, mode 0755: git-ignored, inside
// the checkout (so a worktree's removal takes it), and outside `api/target`,
// which `talaria seed-target` refuses to fill once it exists. A file already
// there is reused without a network call; writing one removes the others.
//
// GITHUB OVER REST, NOT gh — gate:remote's rules (scripts/hooks/
// remote-gate.mjs): the token is GH_TOKEN, else GITHUB_TOKEN, else `gh auth
// token`; the repository is origin's; the zip's redirect to signed storage on
// another host is followed by hand, without the token. So it needs no gh —
// but a binary built on ubuntu-latest needs glibc 2.39 (GLIBC_2.38 and 2.39
// symbols, read from the 2026-11-10 build), and the devbox image is bookworm
// (2.36): in a box the smoke run fails and smokeVerdict says to use cargo.
//
// The tests are ci-artifact.test.ts: pure, no network.

import { createHash } from 'node:crypto'
import { spawnSync } from 'node:child_process'
import { chmodSync, mkdirSync, readdirSync, renameSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { inflateRawSync } from 'node:zlib'
import type { Ctx } from './ctx'

/** What the binary is compiled from: api/, and the files outside it that a
 *  crate embeds (`include_str!`). scripts/surfaces.mjs's API_EXTERNAL_INPUTS
 *  is the list the planner keeps; ci-artifact.test.ts holds the two equal. */
export const API_BINARY_INPUTS: readonly string[] = [
  'api/',
  'scripts/chassis.template.yml',
  'scripts/hermes-toolsets.json',
  'scripts/hermes-skill-authority.json',
  'scripts/skills/',
  'mcp/src/index.ts',
]

/** ci.yml's upload step names the artifact this, plus the full commit sha. */
export const ARTIFACT_PREFIX = 'talaria-api-linux-x64-'
export const artifactName = (sha: string): string => `${ARTIFACT_PREFIX}${sha}`

/** Where a fetched binary lives, under the checkout root. */
export const BIN_DIR = 'api/.ci-bin'
export const binPath = (root: string, sha: string): string => join(root, BIN_DIR, sha.slice(0, 12), 'talaria-api')

/** How many first-parent commits, HEAD included, are tried. */
export const WALK_LIMIT = 30

export const TOKEN_HELP =
  'set GH_TOKEN — fine-grained, outcrop-labs/talaria: Actions read, Contents read, Metadata read — or `gh auth login`; in a devbox pass it with `talaria box new <name> --env GH_TOKEN=…`'

/** CI builds the binary on ubuntu-latest, x86_64: nothing else can run it. */
export const CI_BINARY_HOST = { platform: 'linux', arch: 'x64' } as const
export type Host = { platform: string; arch: string }

export function hostMessage(host: Host): string {
  return `CI's binary is linux-x64 and this is ${host.platform}-${host.arch} — use \`cargo run\` (plain \`talaria dev\`) here`
}

export const GLIBC_MESSAGE =
  "CI's binary needs a newer glibc than this environment (a bookworm devbox) — use `cargo run` here or exercise on the host"

// ── GitHub ──────────────────────────────────────────────────────────────────

export type GhAuth = { token: string; source: string }

/** GH_TOKEN, else GITHUB_TOKEN, else gh's own token — gh's order, and
 *  gate:remote's. null when there is none. */
export function tokenFrom(env: Record<string, string | undefined>, fromGh: string | null | undefined): GhAuth | null {
  for (const key of ['GH_TOKEN', 'GITHUB_TOKEN']) {
    const value = String(env[key] ?? '').trim()
    if (value) return { token: value, source: key }
  }
  const gh = String(fromGh ?? '').trim()
  return gh ? { token: gh, source: '`gh auth token`' } : null
}

export type Repo = { host: string; slug: string }

/** A GitHub remote URL (https, ssh, scp-like, or OWNER/REPO) as host + slug;
 *  null when it is none. The same reading as pr-watch.mjs's parseRepo. */
export function parseRepo(text: string): Repo | null {
  const s = String(text ?? '').trim()
  const m =
    /^[a-z][a-z0-9+.-]*:\/\/(?:[^@/]+@)?([^/:]+)(?::\d+)?\/([^/]+)\/([^/]+)/i.exec(s) ||
    /^[^@/\s]+@([^:/]+):([^/]+)\/([^/]+)$/.exec(s) ||
    /^(?:([^/\s]+)\/)?([^/\s]+)\/([^/\s]+)$/.exec(s)
  if (!m) return null
  const name = (m[3] ?? '').replace(/\.git$/i, '')
  if (!m[2] || !name) return null
  return { host: (m[1] || 'github.com').toLowerCase(), slug: `${m[2]}/${name}`.toLowerCase() }
}

export function apiBase(host: string): string {
  return host === 'github.com' ? 'https://api.github.com' : `https://${host}/api/v3`
}

export type Gh = { base: string; slug: string; token: string; source: string }
export type FetchLike = (url: string, init?: RequestInit) => Promise<Response>

export type Artifact = {
  id: number
  name: string
  size_in_bytes?: number
  expired?: boolean
  created_at?: string
  digest?: string | null
  workflow_run?: {
    id?: number
    repository_id?: number
    head_repository_id?: number
    head_branch?: string
    head_sha?: string
  } | null
}

function headers(gh: Gh): Record<string, string> {
  return {
    authorization: `Bearer ${gh.token}`,
    accept: 'application/vnd.github+json',
    'x-github-api-version': '2022-11-28',
    'user-agent': 'talaria-cli',
  }
}

function errText(e: unknown): string {
  if (e instanceof Error && e.name === 'TimeoutError') return 'no answer in time'
  return e instanceof Error ? e.message : String(e)
}

async function refused(gh: Gh, what: string, res: Response): Promise<Error> {
  let message = (await res.text().catch(() => '')).slice(0, 200)
  try {
    message = (JSON.parse(message) as { message?: string }).message || message
  } catch {
    // not JSON: the raw start of the body says enough
  }
  const scope = res.status === 401 || res.status === 403 ? ` — the token from ${gh.source} was refused; ${TOKEN_HELP}` : ''
  return new Error(`${what}: GitHub answered ${res.status} ${message}${scope}`)
}

/** The artifact to trust for `sha`, of those GitHub listed under its name:
 *  unexpired, made by a run of this repository (not a fork's pull request)
 *  for exactly that commit, the newest first. */
export function pickArtifact(list: readonly Artifact[], sha: string): Artifact | null {
  const name = artifactName(sha)
  const trusted = list.filter((a) => {
    const run = a.workflow_run
    return (
      a.name === name &&
      a.expired !== true &&
      run?.head_sha === sha &&
      typeof run.repository_id === 'number' &&
      run.repository_id === run.head_repository_id
    )
  })
  const at = (a: Artifact) => Date.parse(a.created_at ?? '') || 0
  trusted.sort((a, b) => at(b) - at(a))
  return trusted[0] ?? null
}

/** The trusted artifact CI uploaded for `sha`, or null when there is none. */
export async function findArtifact(gh: Gh, sha: string, fetchImpl: FetchLike = fetch): Promise<Artifact | null> {
  const q = new URLSearchParams({ name: artifactName(sha), per_page: '10' })
  const path = `/repos/${gh.slug}/actions/artifacts?${q}`
  let res: Response
  try {
    res = await fetchImpl(`${gh.base}${path}`, { headers: headers(gh), signal: AbortSignal.timeout(20_000) })
  } catch (e) {
    throw new Error(`GET ${path}: ${errText(e)}`)
  }
  if (!res.ok) throw await refused(gh, `GET ${path}`, res)
  const body = (await res.json().catch(() => null)) as { artifacts?: Artifact[] } | null
  return pickArtifact(body?.artifacts ?? [], sha)
}

/** The artifact's zip. GitHub answers with a redirect to signed storage on
 *  another host, followed by hand so the token never goes there. The bytes
 *  must hash to the artifact's `digest` when GitHub gives one. */
export async function downloadArtifact(gh: Gh, artifact: Artifact, fetchImpl: FetchLike = fetch): Promise<Uint8Array> {
  const path = `/repos/${gh.slug}/actions/artifacts/${artifact.id}/zip`
  let blob: Response
  try {
    const res = await fetchImpl(`${gh.base}${path}`, { headers: headers(gh), redirect: 'manual', signal: AbortSignal.timeout(20_000) })
    if (res.status >= 300 && res.status < 400) {
      const location = res.headers.get('location')
      if (!location) throw new Error(`GitHub answered ${res.status} with no location`)
      blob = await fetchImpl(location, { signal: AbortSignal.timeout(15 * 60_000) })
      if (!blob.ok) throw new Error(`artifact storage answered ${blob.status}`)
    } else if (res.ok) {
      blob = res
    } else {
      throw await refused(gh, `GET ${path}`, res)
    }
  } catch (e) {
    throw new Error(`${artifact.name}: the download failed — ${errText(e)}`)
  }
  const bytes = new Uint8Array(await blob.arrayBuffer())
  const want = /^sha256:([0-9a-f]{64})$/i.exec(artifact.digest ?? '')?.[1]?.toLowerCase()
  if (want) {
    const got = createHash('sha256').update(bytes).digest('hex')
    if (got !== want) throw new Error(`${artifact.name}: the download's sha256 is ${got}, not the artifact's ${want}`)
  }
  return bytes
}

// ── The zip ─────────────────────────────────────────────────────────────────

let CRC_TABLE: Uint32Array | null = null

/** The zip format's CRC-32 (IEEE). */
export function crc32(bytes: Uint8Array): number {
  const table =
    CRC_TABLE ??
    (CRC_TABLE = (() => {
      const t = new Uint32Array(256)
      for (let n = 0; n < 256; n++) {
        let c = n
        for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1
        t[n] = c >>> 0
      }
      return t
    })())
  let c = 0xffffffff
  for (let i = 0; i < bytes.length; i++) c = table[(c ^ bytes[i]!) & 0xff]! ^ (c >>> 8)
  return (c ^ 0xffffffff) >>> 0
}

/**
 * The one file in a zip archive, stored (method 0) or deflated (method 8).
 * Its sizes and CRC come from the central directory: actions/upload-artifact
 * streams the entry, so the local header carries zeros and a data descriptor
 * follows the data (general-purpose flag bit 3). Throws on more than one
 * entry, zip64, encryption, another method, a truncated archive, or a CRC or
 * size that does not match.
 */
export function extractSingle(zip: Uint8Array): { name: string; data: Uint8Array } {
  const buf = Buffer.from(zip.buffer, zip.byteOffset, zip.byteLength)
  const fail = (why: string): never => {
    throw new Error(`the artifact zip: ${why}`)
  }
  // The end-of-central-directory record: 22 bytes, then a comment of at most
  // 64 KiB, so it is searched for from the end.
  let eocd = -1
  for (let i = buf.length - 22; i >= Math.max(0, buf.length - 22 - 0xffff); i--) {
    if (buf.readUInt32LE(i) === 0x06054b50) {
      eocd = i
      break
    }
  }
  if (eocd < 0) fail('no end-of-central-directory record (not a zip)')
  const entries = buf.readUInt16LE(eocd + 10)
  const cd = buf.readUInt32LE(eocd + 16)
  if (entries === 0xffff || cd === 0xffffffff) fail('zip64, which this reader does not take')
  if (entries !== 1) fail(`${entries} entries, where one binary was expected`)
  if (cd + 46 > buf.length || buf.readUInt32LE(cd) !== 0x02014b50) fail('no central directory header')
  const flags = buf.readUInt16LE(cd + 8)
  const method = buf.readUInt16LE(cd + 10)
  const crc = buf.readUInt32LE(cd + 16)
  const packed = buf.readUInt32LE(cd + 20)
  const size = buf.readUInt32LE(cd + 24)
  const nameLen = buf.readUInt16LE(cd + 28)
  const local = buf.readUInt32LE(cd + 42)
  const name = buf.toString('utf8', cd + 46, cd + 46 + nameLen)
  if (flags & 1) fail(`${name} is encrypted`)
  if (packed === 0xffffffff || size === 0xffffffff || local === 0xffffffff) fail('zip64, which this reader does not take')
  if (local + 30 > buf.length || buf.readUInt32LE(local) !== 0x04034b50) fail(`no local header for ${name}`)
  const start = local + 30 + buf.readUInt16LE(local + 26) + buf.readUInt16LE(local + 28)
  if (start + packed > buf.length) fail(`${name} is truncated`)
  const raw = buf.subarray(start, start + packed)
  let data: Uint8Array
  if (method === 0) data = Uint8Array.from(raw)
  else if (method === 8) data = inflateRawSync(raw)
  else return fail(`${name} uses compression method ${method}; only stored (0) and deflate (8) are read`)
  if (data.length !== size) fail(`${name} inflated to ${data.length} bytes, not ${size}`)
  if (crc32(data) !== crc) fail(`${name} fails its CRC`)
  return { name, data }
}

// ── Which commit's binary ───────────────────────────────────────────────────

/**
 * The commits whose binary this tree can run, in the order they are tried:
 * HEAD, then its first parents (`walk`, newest first, HEAD's own row
 * optional) for as long as each one's api inputs match HEAD's. The first
 * commit with another api ends it: a branch that moved the api never runs
 * its base's binary, which is the code before the change.
 */
export function binaryCandidates(head: string, walk: readonly { sha: string; sameApi: boolean }[]): string[] {
  const out = [head]
  for (const c of walk) {
    if (c.sha === head) continue
    if (!c.sameApi) break
    if (!out.includes(c.sha)) out.push(c.sha)
  }
  return out
}

// ── Does it run here ────────────────────────────────────────────────────────

/** The hidden argv mode the binary answers before it reads any config
 *  (api/src/main.rs, talaria-fitness-code-runner's CHILD_PROBE_ARG). With
 *  empty stdin it writes `bad probe ask` and exits 2: no runtime, no port, no
 *  database. The binary has no --version, and an unknown argument would boot
 *  the server. */
export const SMOKE_ARG = '__code-probe'

export type SmokeRun = { status: number | null; stdout: string; stderr: string; error?: Error }

/** Run `<bin> __code-probe` with empty stdin and an empty environment, so
 *  even a binary without that mode stops at its config check
 *  ("DATABASE_URL is required") instead of serving. */
export function smokeRun(bin: string): SmokeRun {
  const r = spawnSync(bin, [SMOKE_ARG], { input: '', env: {}, encoding: 'utf8', timeout: 30_000 })
  return { status: r.status, stdout: String(r.stdout ?? ''), stderr: String(r.stderr ?? ''), error: r.error }
}

/** Whether the smoke run shows a binary that loads and runs here. */
export function smokeVerdict(r: SmokeRun): { ok: true } | { ok: false; why: string } {
  if (r.stdout.includes('bad probe ask') || r.stderr.includes('DATABASE_URL')) return { ok: true }
  if (/GLIBC_[0-9.]+'? not found/.test(r.stderr)) return { ok: false, why: GLIBC_MESSAGE }
  if (r.error) {
    const code = (r.error as { code?: unknown }).code
    if (code === 'ETIMEDOUT') return { ok: false, why: "CI's binary did not answer its smoke run in 30 s" }
    return { ok: false, why: `CI's binary could not start here: ${r.error.message}` }
  }
  const first = r.stderr.split('\n').find((l) => l.trim()) ?? r.stdout.split('\n').find((l) => l.trim())
  return { ok: false, why: `CI's binary did not run here (exit ${r.status ?? 'by signal'}${first ? `: ${first.trim()}` : ''})` }
}

// ── The command's half ──────────────────────────────────────────────────────

export type ApiFromCiOpts = {
  fetchImpl?: FetchLike
  smoke?: (bin: string) => SmokeRun
  /** The machine the binary would run on (default: this process's). */
  host?: Host
}

async function git(ctx: Ctx, args: string[]): Promise<string> {
  return (await ctx.exec('git', args, { cwd: ctx.root })).stdout.trim()
}

/** Do the api inputs differ between `from` and `to` (the working tree, when
 *  `to` is omitted)? `git diff --quiet` exits 1 for a difference; anything
 *  else it says is an error. */
async function apiDiffers(ctx: Ctx, from: string, to?: string): Promise<boolean> {
  try {
    await ctx.exec('git', ['diff', '--quiet', from, ...(to ? [to] : []), '--', ...API_BINARY_INPUTS], { cwd: ctx.root })
    return false
  } catch (e) {
    if ((e as { code?: unknown }).code === 1) return true
    throw e
  }
}

function isExecutable(p: string): boolean {
  try {
    const st = statSync(p)
    return st.isFile() && (st.mode & 0o111) !== 0
  } catch {
    return false
  }
}

/**
 * The path of a talaria-api binary CI built from this tree's api inputs,
 * fetched when it is not here yet, and smoke-run. Dies (ctx.log.die) with
 * the next step when there is none, when the api has uncommitted changes, or
 * when the binary cannot run here.
 */
export async function apiFromCi(ctx: Ctx, opts: ApiFromCiOpts = {}): Promise<string> {
  const fetchImpl = opts.fetchImpl ?? fetch
  const smoke = opts.smoke ?? smokeRun
  const die = (msg: string): never => ctx.log.die(msg)

  // Before any git or network: a 50 MiB download that cannot run here helps
  // nobody (a macOS or arm64 checkout).
  const host = opts.host ?? { platform: process.platform, arch: process.arch }
  if (host.platform !== CI_BINARY_HOST.platform || host.arch !== CI_BINARY_HOST.arch) die(hostMessage(host))

  let dirty: boolean
  try {
    dirty = await apiDiffers(ctx, 'HEAD')
  } catch (e) {
    return die(`--api-from-ci: git could not compare the api with HEAD (${errText(e).split('\n')[0]})`)
  }
  if (dirty) {
    die(
      "uncommitted api changes: CI's binary would not contain them — commit and push (bun run gate:remote --full, or the PR), or drop --api-from-ci",
    )
  }

  const head = await git(ctx, ['rev-parse', 'HEAD']).catch((e) => die(`--api-from-ci: git rev-parse HEAD failed (${errText(e)})`))
  const shas = (await git(ctx, ['rev-list', '--first-parent', `--max-count=${WALK_LIMIT}`, 'HEAD']).catch(() => ''))
    .split('\n')
    .map((s) => s.trim())
    .filter(Boolean)
  const walked: { sha: string; sameApi: boolean }[] = []
  for (const sha of shas) {
    const sameApi = sha === head || !(await apiDiffers(ctx, sha, 'HEAD').catch(() => true))
    walked.push({ sha, sameApi })
    if (!sameApi) break
  }
  const candidates = binaryCandidates(head, walked)
  const whose = (sha: string) => {
    if (sha === head) return `HEAD ${sha.slice(0, 12)}`
    const back = shas.indexOf(sha)
    return `${sha.slice(0, 12)} (HEAD~${back}, the same api as HEAD)`
  }

  const ready = (bin: string, sha: string): string => {
    const verdict = smokeVerdict(smoke(bin))
    if (!verdict.ok) die(`${verdict.why}\n  (binary: ${bin})`)
    ctx.log.say(`rust api → CI's binary for ${whose(sha)}`)
    return bin
  }

  // A binary already fetched for any candidate: they are the same build.
  for (const sha of candidates) {
    const bin = binPath(ctx.root, sha)
    if (isExecutable(bin)) return ready(bin, sha)
  }

  let originUrl = ''
  try {
    originUrl = await git(ctx, ['remote', 'get-url', 'origin'])
  } catch (e) {
    die(`--api-from-ci: no origin remote (${errText(e)})`)
  }
  const repo = parseRepo(originUrl) ?? die(`--api-from-ci: origin is not a GitHub repository (${originUrl})`)
  let fromGh: string | null = null
  if (!ctx.env.GH_TOKEN?.trim() && !ctx.env.GITHUB_TOKEN?.trim()) {
    fromGh = await ctx
      .exec('gh', ['auth', 'token', '--hostname', repo.host], { timeoutMs: 15_000 })
      .then((r) => r.stdout.trim())
      .catch(() => null)
  }
  const auth = tokenFrom(ctx.env, fromGh) ?? die(`--api-from-ci: no GitHub token: ${TOKEN_HELP}`)
  const gh: Gh = { base: apiBase(repo.host), slug: repo.slug, token: auth.token, source: auth.source }

  for (const sha of candidates) {
    let artifact: Artifact | null
    try {
      artifact = await findArtifact(gh, sha, fetchImpl)
    } catch (e) {
      return die(`--api-from-ci: ${errText(e)}`)
    }
    if (!artifact) continue
    const mb = artifact.size_in_bytes ? ` (${Math.round(artifact.size_in_bytes / 1048576)} MiB zipped)` : ''
    ctx.log.say(`fetching ${artifact.name}${mb} from CI run ${artifact.workflow_run?.id ?? '?'}`)
    let data: Uint8Array
    try {
      data = extractSingle(await downloadArtifact(gh, artifact, fetchImpl)).data
    } catch (e) {
      return die(`--api-from-ci: ${errText(e)}`)
    }
    const bin = binPath(ctx.root, sha)
    mkdirSync(dirname(bin), { recursive: true })
    const part = `${bin}.part-${process.pid}`
    writeFileSync(part, data, { mode: 0o755 })
    chmodSync(part, 0o755)
    renameSync(part, bin)
    // One binary per checkout: the others are older builds of this tree.
    const keep = sha.slice(0, 12)
    for (const entry of readdirSync(join(ctx.root, BIN_DIR))) {
      if (entry !== keep) rmSync(join(ctx.root, BIN_DIR, entry), { recursive: true, force: true })
    }
    return ready(bin, sha)
  }

  const changedAt = walked.find((w) => !w.sameApi)
  const tried =
    (candidates.length > 1 ? `HEAD and the ${candidates.length - 1} commit(s) before it with the same api` : 'HEAD only') +
    (changedAt ? `; ${changedAt.sha.slice(0, 12)} and older have another api` : '')
  return die(
    `no CI binary for ${head.slice(0, 12)}: push it with \`bun run gate:remote --full\` (a light run builds the binary only when talaria-api is in scope) or open the PR, then retry\n` +
      `  (tried ${tried}; CI keeps a binary three days)`,
  )
}
