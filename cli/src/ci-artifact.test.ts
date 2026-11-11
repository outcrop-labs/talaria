// `talaria dev --api-from-ci`'s half: the zip reader, which artifact is
// trusted, the download, which commit's binary, the smoke verdict, and the
// whole fetch against a planted git, a fake GitHub and a tmp checkout. No
// network, no binary run.

import { describe, expect, test } from 'bun:test'
import { createHash } from 'node:crypto'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { deflateRawSync } from 'node:zlib'
import {
  API_BINARY_INPUTS,
  BIN_DIR,
  CI_BINARY_HOST,
  GLIBC_MESSAGE,
  apiFromCi,
  artifactName,
  binPath,
  binaryCandidates,
  crc32,
  downloadArtifact,
  extractSingle,
  findArtifact,
  hostMessage,
  parseRepo,
  pickArtifact,
  smokeVerdict,
  tokenFrom,
  type Artifact,
  type FetchLike,
  type Gh,
  type SmokeRun,
} from './ci-artifact'
import { fakeCtx, type FakeCtx } from './testing'
import { CliError } from './ui'

// ── zip fixtures ────────────────────────────────────────────────────────────

type Entry = { name: string; data: Uint8Array; method: 0 | 8; descriptor?: boolean }

/** A zip as actions/upload-artifact writes one: with `descriptor`, the local
 *  header carries zeros for the CRC and sizes (general-purpose bit 3) and a
 *  data descriptor follows the data; the central directory has the truth. */
function zip(entries: Entry[], { badCrc = false } = {}): Uint8Array<ArrayBuffer> {
  const locals: Buffer[] = []
  const centrals: Buffer[] = []
  let offset = 0
  for (const e of entries) {
    const name = Buffer.from(e.name)
    const packed = e.method === 8 ? deflateRawSync(e.data) : Buffer.from(e.data)
    const crc = crc32(e.data) ^ (badCrc ? 1 : 0)
    const flags = e.descriptor ? 0x8 : 0
    const lh = Buffer.alloc(30)
    lh.writeUInt32LE(0x04034b50, 0)
    lh.writeUInt16LE(20, 4)
    lh.writeUInt16LE(flags, 6)
    lh.writeUInt16LE(e.method, 8)
    lh.writeUInt32LE(e.descriptor ? 0 : crc >>> 0, 14)
    lh.writeUInt32LE(e.descriptor ? 0 : packed.length, 18)
    lh.writeUInt32LE(e.descriptor ? 0 : e.data.length, 22)
    lh.writeUInt16LE(name.length, 26)
    lh.writeUInt16LE(0, 28)
    const dd = e.descriptor ? Buffer.alloc(16) : Buffer.alloc(0)
    if (e.descriptor) {
      dd.writeUInt32LE(0x08074b50, 0)
      dd.writeUInt32LE(crc >>> 0, 4)
      dd.writeUInt32LE(packed.length, 8)
      dd.writeUInt32LE(e.data.length, 12)
    }
    const cd = Buffer.alloc(46)
    cd.writeUInt32LE(0x02014b50, 0)
    cd.writeUInt16LE(20, 4)
    cd.writeUInt16LE(20, 6)
    cd.writeUInt16LE(flags, 8)
    cd.writeUInt16LE(e.method, 10)
    cd.writeUInt32LE(crc >>> 0, 16)
    cd.writeUInt32LE(packed.length, 20)
    cd.writeUInt32LE(e.data.length, 24)
    cd.writeUInt16LE(name.length, 28)
    cd.writeUInt32LE(offset, 42)
    const local = Buffer.concat([lh, name, packed, dd])
    locals.push(local)
    centrals.push(Buffer.concat([cd, name]))
    offset += local.length
  }
  const cdBuf = Buffer.concat(centrals)
  const eocd = Buffer.alloc(22)
  eocd.writeUInt32LE(0x06054b50, 0)
  eocd.writeUInt16LE(entries.length, 8)
  eocd.writeUInt16LE(entries.length, 10)
  eocd.writeUInt32LE(cdBuf.length, 12)
  eocd.writeUInt32LE(offset, 16)
  return new Uint8Array(Buffer.concat([...locals, cdBuf, eocd]))
}

const BINARY = new Uint8Array(Buffer.from('\x7fELF talaria-api '.repeat(4000)))

describe('extractSingle', () => {
  test('a stored entry', () => {
    const out = extractSingle(zip([{ name: 'talaria-api', data: BINARY, method: 0 }]))
    expect(out.name).toBe('talaria-api')
    expect(Buffer.from(out.data).equals(Buffer.from(BINARY))).toBe(true)
  })

  test('a deflated entry', () => {
    const out = extractSingle(zip([{ name: 'talaria-api', data: BINARY, method: 8 }]))
    expect(Buffer.from(out.data).equals(Buffer.from(BINARY))).toBe(true)
  })

  test("upload-artifact's shape: zeros in the local header and a data descriptor — the sizes come from the central directory", () => {
    const out = extractSingle(zip([{ name: 'talaria-api', data: BINARY, method: 8, descriptor: true }]))
    expect(Buffer.from(out.data).equals(Buffer.from(BINARY))).toBe(true)
  })

  test('more than one entry is refused', () => {
    const two = zip([
      { name: 'talaria-api', data: BINARY, method: 8 },
      { name: 'other', data: BINARY, method: 0 },
    ])
    expect(() => extractSingle(two)).toThrow('2 entries, where one binary was expected')
  })

  test('another compression method is refused', () => {
    const z = zip([{ name: 'talaria-api', data: BINARY, method: 0 }])
    const buf = Buffer.from(z)
    const cd = buf.readUInt32LE(buf.length - 22 + 16)
    buf.writeUInt16LE(12, cd + 10) // bzip2
    expect(() => extractSingle(new Uint8Array(buf))).toThrow('compression method 12')
  })

  test('a CRC that does not match, a truncated archive, and not a zip at all', () => {
    expect(() => extractSingle(zip([{ name: 'talaria-api', data: BINARY, method: 8 }], { badCrc: true }))).toThrow('fails its CRC')
    const z = zip([{ name: 'talaria-api', data: BINARY, method: 0 }])
    expect(() => extractSingle(z.subarray(0, 4000))).toThrow()
    expect(() => extractSingle(new Uint8Array(Buffer.from('not a zip, just some bytes')))).toThrow('not a zip')
  })

  test('crc32 is the zip one', () => {
    expect(crc32(new Uint8Array(Buffer.from('123456789')))).toBe(0xcbf43926)
  })
})

// ── GitHub ──────────────────────────────────────────────────────────────────

const REPO_ID = 1286764077
const SHA = 'a'.repeat(40)
const MB = 'b'.repeat(40)
const OLDER = 'c'.repeat(40)
const art = (over: Partial<Artifact> & { sha?: string; run?: Partial<NonNullable<Artifact['workflow_run']>> } = {}): Artifact => {
  const sha = over.sha ?? SHA
  return {
    id: 1,
    name: artifactName(sha),
    size_in_bytes: 1024,
    expired: false,
    created_at: '2026-11-10T10:00:00Z',
    ...over,
    workflow_run: { id: 77, repository_id: REPO_ID, head_repository_id: REPO_ID, head_branch: 'wt/x', head_sha: sha, ...over.run },
  }
}

const GH: Gh = { base: 'https://api.github.com', slug: 'outcrop-labs/talaria', token: 'tok', source: 'GH_TOKEN' }

describe('which artifact is trusted', () => {
  test('the newest unexpired one, of this repository, for exactly that commit', () => {
    const list = [
      art({ id: 1, created_at: '2026-11-10T09:00:00Z' }),
      art({ id: 2, created_at: '2026-11-10T11:00:00Z' }),
      art({ id: 3, created_at: '2026-11-10T12:00:00Z', expired: true }),
    ]
    expect(pickArtifact(list, SHA)?.id).toBe(2)
  })

  test("a fork's pull request run is never trusted — it could upload any file under any name", () => {
    expect(pickArtifact([art({ run: { head_repository_id: 999 } })], SHA)).toBeNull()
    expect(pickArtifact([art({ run: { repository_id: undefined, head_repository_id: undefined } })], SHA)).toBeNull()
  })

  test('an artifact whose run was for another commit, or another name, is not this one', () => {
    expect(pickArtifact([art({ run: { head_sha: MB } })], SHA)).toBeNull()
    expect(pickArtifact([art({ name: 'talaria-api-linux-x64-other' })], SHA)).toBeNull()
  })

  test('findArtifact asks GitHub for the exact name with the token, and picks from the answer', async () => {
    const asked: { url: string; auth?: string }[] = []
    const fetchImpl: FetchLike = async (url, init) => {
      asked.push({ url, auth: (init?.headers as Record<string, string>)?.authorization })
      return Response.json({ total_count: 2, artifacts: [art({ id: 5, created_at: '2026-11-10T08:00:00Z' }), art({ id: 6 })] })
    }
    expect((await findArtifact(GH, SHA, fetchImpl))?.id).toBe(6)
    expect(asked).toEqual([
      { url: `https://api.github.com/repos/outcrop-labs/talaria/actions/artifacts?name=talaria-api-linux-x64-${SHA}&per_page=10`, auth: 'Bearer tok' },
    ])
  })

  test('a refused token names where it came from and what it needs', async () => {
    const fetchImpl: FetchLike = async () => Response.json({ message: 'Bad credentials' }, { status: 401 })
    await expect(findArtifact(GH, SHA, fetchImpl)).rejects.toThrow('the token from GH_TOKEN was refused; set GH_TOKEN')
  })
})

describe('downloadArtifact', () => {
  const bytes = zip([{ name: 'talaria-api', data: BINARY, method: 8, descriptor: true }])
  const digest = `sha256:${createHash('sha256').update(bytes).digest('hex')}`

  test("follows GitHub's redirect to storage by hand, without the token, and checks the digest", async () => {
    const asked: { url: string; auth?: string; redirect?: string }[] = []
    const fetchImpl: FetchLike = async (url, init) => {
      asked.push({ url, auth: (init?.headers as Record<string, string> | undefined)?.authorization, redirect: init?.redirect })
      if (url.startsWith('https://api.github.com/')) return new Response(null, { status: 302, headers: { location: 'https://blob.example/signed?sig=1' } })
      return new Response(bytes)
    }
    const got = await downloadArtifact(GH, art({ id: 42, digest }), fetchImpl)
    expect(Buffer.from(got).equals(Buffer.from(bytes))).toBe(true)
    expect(asked).toEqual([
      { url: 'https://api.github.com/repos/outcrop-labs/talaria/actions/artifacts/42/zip', auth: 'Bearer tok', redirect: 'manual' },
      { url: 'https://blob.example/signed?sig=1', auth: undefined, redirect: undefined },
    ])
  })

  test('bytes that do not hash to the digest are refused', async () => {
    const fetchImpl: FetchLike = async () => new Response(bytes)
    await expect(downloadArtifact(GH, art({ digest: `sha256:${'0'.repeat(64)}` }), fetchImpl)).rejects.toThrow("not the artifact's")
  })

  test('an expired artifact (410) fails with what GitHub said', async () => {
    const fetchImpl: FetchLike = async () => Response.json({ message: 'Artifact has expired' }, { status: 410 })
    await expect(downloadArtifact(GH, art(), fetchImpl)).rejects.toThrow('GitHub answered 410 Artifact has expired')
  })
})

describe('tokens and the repository', () => {
  test("GH_TOKEN, then GITHUB_TOKEN, then gh's — gate:remote's order", () => {
    expect(tokenFrom({ GH_TOKEN: 'a', GITHUB_TOKEN: 'b' }, 'c')).toEqual({ token: 'a', source: 'GH_TOKEN' })
    expect(tokenFrom({ GH_TOKEN: ' ', GITHUB_TOKEN: 'b' }, 'c')).toEqual({ token: 'b', source: 'GITHUB_TOKEN' })
    expect(tokenFrom({}, 'c\n')).toEqual({ token: 'c', source: '`gh auth token`' })
    expect(tokenFrom({}, null)).toBeNull()
  })

  test('origin, spelled any way git spells it', () => {
    const want = { host: 'github.com', slug: 'outcrop-labs/talaria' }
    expect(parseRepo('https://github.com/outcrop-labs/talaria.git')).toEqual(want)
    expect(parseRepo('git@github.com:outcrop-labs/talaria.git')).toEqual(want)
    expect(parseRepo('ssh://git@github.com/outcrop-labs/talaria')).toEqual(want)
    expect(parseRepo('/srv/git/talaria.git/')).toBeNull()
  })
})

// ── which commit ────────────────────────────────────────────────────────────

describe('binaryCandidates — which commits share HEAD\'s binary', () => {
  test('a branch whose last commit changed the api runs only its own binary', () => {
    expect(binaryCandidates(SHA, [{ sha: SHA, sameApi: true }, { sha: MB, sameApi: false }])).toEqual([SHA])
  })

  test('a branch that did not change the api: HEAD, then the merge-base (a testing push binary)', () => {
    expect(binaryCandidates(SHA, [{ sha: SHA, sameApi: true }, { sha: MB, sameApi: true }])).toEqual([SHA, MB])
  })

  test('then further back, to the last api change and no further', () => {
    const walk = [
      { sha: SHA, sameApi: true },
      { sha: MB, sameApi: true },
      { sha: OLDER, sameApi: true },
      { sha: 'd'.repeat(40), sameApi: false },
      { sha: 'e'.repeat(40), sameApi: true },
    ]
    expect(binaryCandidates(SHA, walk)).toEqual([SHA, MB, OLDER])
  })

  test('HEAD is first and tried once, whether or not the walk lists it', () => {
    expect(binaryCandidates(MB, [{ sha: MB, sameApi: true }])).toEqual([MB])
    expect(binaryCandidates(SHA, [])).toEqual([SHA])
  })

  test("the inputs are api/ and the planner's external api inputs, kept equal to scripts/surfaces.mjs", async () => {
    const surfaces = (await import(join(import.meta.dir, '../../scripts/surfaces.mjs'))) as { API_EXTERNAL_INPUTS: string[] }
    expect([...API_BINARY_INPUTS]).toEqual(['api/', ...surfaces.API_EXTERNAL_INPUTS])
  })
})

describe('smokeVerdict', () => {
  const run = (over: Partial<SmokeRun>): SmokeRun => ({ status: 0, stdout: '', stderr: '', ...over })

  test('the probe mode answering, or a binary without it stopping at its config, both run', () => {
    expect(smokeVerdict(run({ status: 2, stdout: '{"Error":"bad probe ask: EOF while parsing a value at line 1 column 0"}\n' }))).toEqual({ ok: true })
    expect(smokeVerdict(run({ status: 1, stderr: 'DATABASE_URL is required (postgres://…)\n' }))).toEqual({ ok: true })
  })

  test("a glibc too old for CI's build says so, and what to do instead", () => {
    const stderr = "/w/api/.ci-bin/aaaa/talaria-api: /lib/x86_64-linux-gnu/libc.so.6: version `GLIBC_2.39' not found (required by /w/api/.ci-bin/aaaa/talaria-api)\n"
    expect(smokeVerdict(run({ status: 1, stderr }))).toEqual({ ok: false, why: GLIBC_MESSAGE })
  })

  test('anything else is a failure that quotes the binary', () => {
    expect(smokeVerdict(run({ status: 139, stderr: 'Segmentation fault\n' }))).toEqual({
      ok: false,
      why: "CI's binary did not run here (exit 139: Segmentation fault)",
    })
    const timedOut = Object.assign(new Error('spawnSync ETIMEDOUT'), { code: 'ETIMEDOUT' })
    expect(smokeVerdict(run({ status: null, error: timedOut }))).toEqual({ ok: false, why: "CI's binary did not answer its smoke run in 30 s" })
  })
})

// ── the whole fetch ─────────────────────────────────────────────────────────

const DIFF = (from: string, to?: string) => ['diff', '--quiet', from, ...(to ? [to] : []), '--', ...API_BINARY_INPUTS]
const differs = () => Object.assign(new Error('exit 1'), { code: 1 })
const OK_SMOKE = (): SmokeRun => ({ status: 2, stdout: '{"Error":"bad probe ask: EOF"}', stderr: '' })

/** A checkout on disk (for api/.ci-bin), git planted: HEAD = SHA, whose
 *  first parents are MB (where the branch left testing) and then OLDER. */
function checkout(env: Record<string, string> = { GH_TOKEN: 'tok' }): { ctx: FakeCtx; root: string; done: () => void } {
  const root = mkdtempSync(join(tmpdir(), 'talaria-ci-artifact-'))
  const ctx = fakeCtx({ env })
  ctx.root = root
  ctx.plant(['git', ['rev-parse', 'HEAD']], `${SHA}\n`)
  ctx.plant(['git', ['rev-list', '--first-parent', '--max-count=30', 'HEAD']], `${SHA}\n${MB}\n${OLDER}\n`)
  ctx.plant(['git', ['remote', 'get-url', 'origin']], 'git@github.com:outcrop-labs/talaria.git\n')
  return { ctx, root, done: () => rmSync(root, { recursive: true, force: true }) }
}

/** GitHub with artifacts for `have` (sha → zip bytes). */
function github(have: Record<string, Uint8Array<ArrayBuffer>>) {
  const asked: string[] = []
  const fetchImpl: FetchLike = async (url) => {
    asked.push(url)
    const name = new URL(url).searchParams.get('name')
    if (name) {
      const sha = name.slice('talaria-api-linux-x64-'.length)
      const bytes = have[sha]
      const digest = bytes ? `sha256:${createHash('sha256').update(bytes).digest('hex')}` : null
      return Response.json({ artifacts: bytes ? [art({ sha, id: sha.charCodeAt(0), digest })] : [] })
    }
    const id = /artifacts\/(\d+)\/zip$/.exec(url)?.[1]
    if (id) return new Response(null, { status: 302, headers: { location: `https://blob.example/${id}` } })
    const blob = /blob\.example\/(\d+)$/.exec(url)?.[1]
    const sha = Object.keys(have).find((s) => String(s.charCodeAt(0)) === blob)
    return sha ? new Response(have[sha]) : new Response('gone', { status: 404 })
  }
  return { fetchImpl, asked }
}

const attempt = async (p: Promise<string>): Promise<string> => {
  try {
    return await p
  } catch (e) {
    if (e instanceof CliError) return `die: ${e.message}`
    throw e
  }
}

describe('apiFromCi', () => {
  const payload = zip([{ name: 'talaria-api', data: BINARY, method: 8, descriptor: true }])

  test("uncommitted api changes are refused before anything is fetched: CI's binary would not contain them", async () => {
    const { ctx, done } = checkout()
    ctx.plant(['git', DIFF('HEAD')], differs())
    const { fetchImpl, asked } = github({ [SHA]: payload })
    try {
      const out = await attempt(apiFromCi(ctx, { fetchImpl, smoke: OK_SMOKE, host: CI_BINARY_HOST }))
      expect(out).toBe(
        "die: uncommitted api changes: CI's binary would not contain them — commit and push (bun run gate:remote --full, or the PR), or drop --api-from-ci",
      )
      expect(asked).toEqual([])
    } finally {
      done()
    }
  })

  test("HEAD's own binary: fetched into api/.ci-bin/<sha12>/talaria-api, executable, smoke-run — and reused next time without GitHub", async () => {
    const { ctx, root, done } = checkout()
    ctx.plant(['git', DIFF(MB, 'HEAD')], differs()) // the branch changed the api
    const { fetchImpl, asked } = github({ [SHA]: payload })
    const smoked: string[] = []
    const smoke = (bin: string) => (smoked.push(bin), OK_SMOKE())
    try {
      const bin = await apiFromCi(ctx, { fetchImpl, smoke, host: CI_BINARY_HOST })
      expect(bin).toBe(join(root, BIN_DIR, SHA.slice(0, 12), 'talaria-api'))
      expect(Buffer.from(readFileSync(bin)).equals(Buffer.from(BINARY))).toBe(true)
      expect(statSync(bin).mode & 0o777).toBe(0o755)
      expect(smoked).toEqual([bin])
      expect(asked.length).toBe(3) // find, the redirect, the blob
      expect(ctx.logLines.at(-1)).toEqual({ kind: 'say', msg: `rust api → CI's binary for HEAD ${SHA.slice(0, 12)}` })

      const again = await apiFromCi(ctx, { fetchImpl, smoke, host: CI_BINARY_HOST })
      expect(again).toBe(bin)
      expect(asked.length).toBe(3)
    } finally {
      done()
    }
  })

  test("no binary for HEAD, and no api change since the branch left its base: the base's last binary, walking back while the api matches", async () => {
    const { ctx, root, done } = checkout()
    // MB and OLDER both have HEAD's api; only OLDER's push built a binary.
    const { fetchImpl, asked } = github({ [OLDER]: payload })
    mkdirSync(join(root, BIN_DIR, 'f'.repeat(12)), { recursive: true })
    writeFileSync(join(root, BIN_DIR, 'f'.repeat(12), 'talaria-api'), 'an older build')
    try {
      const bin = await apiFromCi(ctx, { fetchImpl, smoke: OK_SMOKE, host: CI_BINARY_HOST })
      expect(bin).toBe(binPath(root, OLDER))
      expect(asked.filter((u) => u.includes('?name=')).map((u) => new URL(u).searchParams.get('name'))).toEqual([
        artifactName(SHA),
        artifactName(MB),
        artifactName(OLDER),
      ])
      // one binary per checkout: the older build is gone
      expect(readdirSync(join(root, BIN_DIR))).toEqual([OLDER.slice(0, 12)])
      expect(ctx.logLines.at(-1)?.msg).toBe(`rust api → CI's binary for ${OLDER.slice(0, 12)} (HEAD~2, the same api as HEAD)`)
    } finally {
      done()
    }
  })

  test('none anywhere: the command says how to get one', async () => {
    const { ctx, done } = checkout()
    ctx.plant(['git', DIFF(MB, 'HEAD')], differs())
    const { fetchImpl } = github({ [MB]: payload }) // the base has one, but its api is not HEAD's
    try {
      const out = await attempt(apiFromCi(ctx, { fetchImpl, smoke: OK_SMOKE, host: CI_BINARY_HOST }))
      expect(out).toBe(
        `die: no CI binary for ${SHA.slice(0, 12)}: push it with \`bun run gate:remote --full\` (a light run builds the binary only when talaria-api is in scope) or open the PR, then retry\n` +
          `  (tried HEAD only; ${MB.slice(0, 12)} and older have another api; CI keeps a binary three days)`,
      )
    } finally {
      done()
    }
  })

  test("a binary this environment's glibc cannot load stops the command with the way round it", async () => {
    const { ctx, done } = checkout()
    const { fetchImpl } = github({ [SHA]: payload })
    const smoke = (): SmokeRun => ({ status: 1, stdout: '', stderr: "x: version `GLIBC_2.39' not found (required by x)" })
    try {
      const out = await attempt(apiFromCi(ctx, { fetchImpl, smoke, host: CI_BINARY_HOST }))
      expect(out.startsWith(`die: ${GLIBC_MESSAGE}\n`)).toBe(true)
    } finally {
      done()
    }
  })

  test('a machine that is not linux-x64 is told so before git or GitHub is asked anything', async () => {
    for (const host of [{ platform: 'darwin', arch: 'arm64' }, { platform: 'linux', arch: 'arm64' }, { platform: 'win32', arch: 'x64' }]) {
      const { ctx, done } = checkout()
      const { fetchImpl, asked } = github({ [SHA]: payload })
      try {
        const out = await attempt(apiFromCi(ctx, { fetchImpl, smoke: OK_SMOKE, host }))
        expect(out).toBe(`die: ${hostMessage(host)}`)
        expect(out).toContain(`this is ${host.platform}-${host.arch}`)
        expect(asked).toEqual([])
        expect(ctx.calls).toEqual([])
      } finally {
        done()
      }
    }
  })

  test("no token anywhere (no GH_TOKEN, no GITHUB_TOKEN, gh says nothing): it says what to set, before asking GitHub", async () => {
    const { ctx, done } = checkout({})
    ctx.plant(['gh', ['auth', 'token', '--hostname', 'github.com']], new Error('not logged in'))
    const { fetchImpl, asked } = github({ [SHA]: payload })
    try {
      const out = await attempt(apiFromCi(ctx, { fetchImpl, smoke: OK_SMOKE, host: CI_BINARY_HOST }))
      expect(out).toContain('die: --api-from-ci: no GitHub token: set GH_TOKEN')
      expect(asked).toEqual([])
      expect(existsSync(join(ctx.root, BIN_DIR))).toBe(false)
    } finally {
      done()
    }
  })
})
