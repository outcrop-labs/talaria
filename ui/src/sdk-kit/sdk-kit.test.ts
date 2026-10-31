// The SDK kit's parts that run without a kit: argument parsing, where an app
// and its work live, the declarations' rewrite, the tar, the checkers'
// configs, the eval transports and the eval run itself over in-memory
// harnesses. The whole loop — fetch, scaffold, check, build, eval — is
// exercised against a running instance (docs/sdk/outside-the-host.md).
import { execFileSync } from 'node:child_process'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { gunzipSync } from 'node:zlib'
import { afterAll, beforeAll, describe, expect, it } from 'vitest'
import { z } from 'zod'
import { defineHarness } from '@/server/harness/define'
import type { HarnessDeps } from '@/server/harness/run'
import type { TransportRequest } from '@/server/harness/transport'
import { formatBuildReport, triggerReport, type BuildReport } from './build'
import { kitTsconfig, kitVitestConfig } from './check'
import { main, parseArgs } from './cli'
import {
  findEvalSession,
  formatEvalReport,
  gatewayBody,
  gatewayTransport,
  promptDigest,
  readRecording,
  recordedTransport,
  recordingPath,
  runEvals,
} from './eval'
import { KIT_FORMAT, KIT_PATHS, kitVersionOk } from './kit'
import { rewriteHostAliases, sdkSourceClosure, staticSpecifiers, tarGz } from './pack'
import { nameForDir, scaffoldApp } from './scaffold'
import { findAppDir, readAppManifest, slugFor, workDirFor } from './workspace'

let root = ''
beforeAll(() => {
  root = mkdtempSync(join(tmpdir(), 'sdk-kit-'))
})
afterAll(() => rmSync(root, { recursive: true, force: true }))

const req = (content: string, extra: Partial<TransportRequest> = {}): TransportRequest => ({
  model: 'm',
  messages: [{ role: 'user', content }],
  jsonMode: false,
  caller: 'test',
  ...extra,
})

describe('talaria-app arguments', () => {
  it('reads the command first, and --harness by it: a switch on scaffold, a filter on eval', () => {
    expect(parseArgs(['scaffold', 'here', '--harness', '--name', 'Notes'])).toEqual({
      command: 'scaffold',
      positionals: ['here'],
      flags: { harness: true, name: 'Notes' },
    })
    expect(parseArgs(['eval', '--harness', 'tidy-title', '--case=a b', '--gateway'])).toEqual({
      command: 'eval',
      positionals: [],
      flags: { harness: 'tidy-title', case: 'a b', gateway: true },
    })
  })

  it('refuses what it does not know, and a value flag with no value', () => {
    expect(parseArgs(['build', '--frobnicate'])).toBe('unknown flag --frobnicate')
    expect(parseArgs(['eval', '--model'])).toBe('--model needs a value')
    expect(parseArgs(['eval', '--model', '--gateway'])).toBe('--model needs a value')
    expect(parseArgs(['build', '--json=yes'])).toBe('--json takes no value')
  })
})

describe('where an app is', () => {
  it('finds talaria.json at or above a directory, and the slug an install would give it', () => {
    const app = join(root, 'talaria-app-field-notes')
    mkdirSync(join(app, 'harnesses'), { recursive: true })
    writeFileSync(join(app, 'talaria.json'), JSON.stringify({ name: 'Field Notes' }))
    expect(findAppDir(join(app, 'harnesses'))).toBe(app)
    expect(findAppDir(tmpdir())).toBeNull()
    // The repository's name, as install_slug reads it…
    expect(slugFor(app, {})).toBe('field-notes')
    // …unless talaria.json declares one, or the caller names one.
    expect(slugFor(app, { slug: 'notes' })).toBe('notes')
    expect(slugFor(app, { slug: 'notes' }, 'other')).toBe('other')
    // A checkout's origin names the repo an install reads, whatever the
    // directory is called (a workbench job clones into a directory named
    // for the job).
    const job = join(root, '6f1c0e2a-job')
    mkdirSync(join(job, '.git'), { recursive: true })
    writeFileSync(join(job, '.git', 'config'), '[core]\n\tbare = false\n[remote "origin"]\n\turl = https://github.com/acme/talaria-app-field-notes.git\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n')
    expect(slugFor(job, {})).toBe('field-notes')
    writeFileSync(join(job, '.git', 'config'), '[remote "origin"]\n\turl = git@github.com:acme/Pulse-Log\n')
    expect(slugFor(job, {})).toBe('pulse-log')
    // A name an install would refuse is no slug at all.
    expect(slugFor(join(root, 'Has Spaces'), {})).toBeNull()
    expect(slugFor(join(root, 'hooks'), {})).toBeNull()
  })

  it('holds talaria.json to the host rules, with the host SDK version', () => {
    const dir = join(root, 'ranged')
    mkdirSync(dir, { recursive: true })
    writeFileSync(join(dir, 'talaria.json'), JSON.stringify({ name: 'Ranged', sdk: '^2' }))
    const read = readAppManifest(dir, '1.0.0')
    expect(read.ok).toBe(false)
    expect(read.ok ? '' : read.error).toContain('^2')
    writeFileSync(join(dir, 'talaria.json'), '{ nope')
    expect(readAppManifest(dir, '1.0.0')).toMatchObject({ ok: false, error: expect.stringContaining('not JSON') })
  })

  it('works outside the app, keyed by its real path', () => {
    const dir = join(root, 'talaria-app-field-notes')
    const work = workDirFor(dir, { HOME: '/home/agent' })
    expect(work.startsWith('/home/agent/.cache/talaria-app/talaria-app-field-notes-')).toBe(true)
    expect(workDirFor(dir, { XDG_CACHE_HOME: '/c', HOME: '/h' }).startsWith('/c/talaria-app/')).toBe(true)
    expect(work.startsWith(dir)).toBe(false)
  })
})

describe('scaffold', () => {
  it("writes the skeleton `talaria app new` writes, at a repo's root, and keeps what the repo had", () => {
    const repo = join(root, 'talaria-app-crm')
    mkdirSync(repo, { recursive: true })
    writeFileSync(join(repo, 'README.md'), '# mine\n')
    writeFileSync(join(repo, '.gitignore'), 'dist/\nnode_modules/\n')
    const done = scaffoldApp(repo, { harness: true })
    if (typeof done === 'string') throw new Error(done)
    expect(done.written.sort()).toEqual(
      ['talaria.json', 'app.ts', 'Work.svelte', 'Item.svelte', 'server.ts', 'mcp.ts', 'harnesses/tidy-title.ts', 'harnesses/recorded/tidy-title.json'].sort(),
    )
    expect(done.kept.sort()).toEqual(['.gitignore', 'README.md'])
    expect(readFileSync(join(repo, 'README.md'), 'utf8')).toBe('# mine\n')
    expect(readFileSync(join(repo, '.gitignore'), 'utf8')).toBe('dist/\nnode_modules/\n.talaria/\n')
    expect(JSON.parse(readFileSync(join(repo, 'talaria.json'), 'utf8')).name).toBe('Crm')
    expect(readFileSync(join(repo, 'harnesses/tidy-title.ts'), 'utf8')).toContain('"crm:tidy-title"')
    // A second scaffold refuses rather than overwrite an app.
    expect(scaffoldApp(repo)).toBe(`${repo} already holds an app (talaria.json)`)
    expect(nameForDir('/x/talaria-app-field-notes')).toBe('Field Notes')
  })

  it('fills in the app around a seed talaria.json, keeping every key the seed declares', () => {
    // A new repo's first file: written so prepare_env (keyed on talaria.json)
    // installs the kit that scaffolds the rest.
    const repo = join(root, 'seeded')
    mkdirSync(repo, { recursive: true })
    writeFileSync(join(repo, 'README.md'), '# seeded\n')
    writeFileSync(join(repo, 'talaria.json'), JSON.stringify({ name: 'Field Notes', slug: 'field-notes', sdk: '^1.0' }))
    const done = scaffoldApp(repo, { harness: true })
    if (typeof done === 'string') throw new Error(done)
    expect(done.written).not.toContain('talaria.json')
    expect(done.written).toContain('app.ts')
    expect(done.kept.sort()).toEqual(['README.md', 'talaria.json'])
    expect(JSON.parse(readFileSync(join(repo, 'talaria.json'), 'utf8'))).toEqual({
      name: 'Field Notes',
      icon: '⬡',
      version: '0.1.0',
      description: 'Field Notes — a Talaria app.',
      surfaces: { work: 'Field Notes' },
      slug: 'field-notes',
      sdk: '^1.0',
    })
    expect(readFileSync(join(repo, 'harnesses/tidy-title.ts'), 'utf8')).toContain('"field-notes:tidy-title"')
    expect(readAppManifest(repo, '1.0.0').ok).toBe(true)
    // Now it is an app: a talaria.json beside the rest is never a seed.
    expect(scaffoldApp(repo)).toBe(`${repo} already holds an app (talaria.json)`)
    // --name wins over the seed's.
    const named = join(root, 'seeded-named')
    mkdirSync(named, { recursive: true })
    writeFileSync(join(named, 'talaria.json'), '{"name":"Old"}')
    const renamed = scaffoldApp(named, { name: 'New' })
    if (typeof renamed === 'string') throw new Error(renamed)
    expect(JSON.parse(readFileSync(join(named, 'talaria.json'), 'utf8')).name).toBe('New')
    // A seed is a JSON object or nothing is written.
    const broken = join(root, 'seeded-broken')
    mkdirSync(broken, { recursive: true })
    writeFileSync(join(broken, 'talaria.json'), '["Notes"]')
    expect(scaffoldApp(broken)).toContain('is not a JSON object')
    writeFileSync(join(broken, 'talaria.json'), '{ nope')
    expect(scaffoldApp(broken)).toContain('is not JSON')
    expect(existsSync(join(broken, 'app.ts'))).toBe(false)
  })

  it("names the app after the repository a workbench job cloned, not the job's directory", () => {
    const job = join(root, '0b6f3c9e-job')
    mkdirSync(join(job, '.git'), { recursive: true })
    writeFileSync(join(job, '.git', 'config'), '[remote "origin"]\n\turl = https://github.com/acme/talaria-app-pulse-log.git\n')
    expect(nameForDir(job)).toBe('Pulse Log')
    const done = scaffoldApp(job)
    if (typeof done === 'string') throw new Error(done)
    expect(JSON.parse(readFileSync(join(job, 'talaria.json'), 'utf8')).name).toBe('Pulse Log')
  })
})

describe('the declarations', () => {
  it('reach the three entries and what they import, never a browser-only module', () => {
    const files = sdkSourceClosure(resolve('src'))
    for (const entry of ['sdk/index.ts', 'sdk/server.ts', 'sdk/testing.ts', 'components/ui/Button.svelte', 'server/harness/define.ts']) {
      expect(files, entry).toContain(entry)
    }
    expect(files).not.toContain('router.ts')
    expect(files.every((f) => !f.endsWith('.test.ts'))).toBe(true)
  })

  it('reads static and type imports from .ts and .svelte, not dynamic ones', () => {
    expect(staticSpecifiers('a.ts', "import type { A } from './a'\nexport { b } from '@/b'\nconst c = () => import('./lazy')\n// import x from './no'\n").sort()).toEqual(['./a', '@/b'])
    expect(staticSpecifiers('A.svelte', "<script lang=\"ts\">\n  import X from './X.svelte'\n</script>\n<X />")).toEqual(['./X.svelte'])
  })

  it('lose every `@/` path: an app cannot name a host internal', () => {
    const dts = "import type { AppStore } from '@/server/app-store';\nexport { HttpError } from '@/lib/fetch-json';\ntype T = import('@/server/harness/run').RunLedger;\nimport '@/styles';\nimport { z } from 'zod';"
    const out = rewriteHostAliases(dts, 'sdk/server.d.ts')
    expect(out).toContain("from '../server/app-store'")
    expect(out).toContain("from '../lib/fetch-json'")
    expect(out).toContain("import('../server/harness/run')")
    expect(out).toContain("import '../styles'")
    expect(out).toContain("from 'zod'")
    expect(out).not.toContain('@/')
    expect(rewriteHostAliases("from '@/sdk/x'", 'sdk/index.d.ts')).toBe("from './x'")
  })
})

describe('the archive', () => {
  it('is a gzipped ustar the system tar reads, byte-identical for the same contents', () => {
    const long = `${'deep/'.repeat(30)}file.d.ts`
    const entries = [
      { path: 'kit.json', data: '{"format":1}\n' },
      { path: 'bin/talaria-app', data: '#!/bin/sh\necho hi\n', mode: 0o755 },
      { path: long, data: new Uint8Array([1, 2, 3]) },
    ]
    const tgz = tarGz(entries)
    expect(tarGz(entries).equals(tgz)).toBe(true)
    const raw = gunzipSync(tgz)
    expect(raw.length % 512).toBe(0)
    const dir = mkdtempSync(join(root, 'tar-'))
    writeFileSync(join(dir, 'k.tgz'), tgz)
    execFileSync('tar', ['-xzf', 'k.tgz'], { cwd: dir })
    expect(readFileSync(join(dir, 'kit.json'), 'utf8')).toBe('{"format":1}\n')
    expect(readFileSync(join(dir, long))).toEqual(Buffer.from([1, 2, 3]))
    expect(execFileSync('sh', ['-c', 'test -x bin/talaria-app && echo yes'], { cwd: dir }).toString().trim()).toBe('yes')
    expect(() => tarGz([{ path: '../escape', data: '' }])).toThrow(/refusing path/)
  })

  it('names its versions in a shape a directory can take', () => {
    expect(kitVersionOk('dev-a962c3625153-4149a895')).toBe(true)
    expect(kitVersionOk('1.4.0+rc.2-0011aabb')).toBe(true)
    for (const bad of ['', '../x', 'a/b', '-x', 'a b', `a${'b'.repeat(90)}`]) expect(kitVersionOk(bad), bad).toBe(false)
  })
})

describe('check', () => {
  it('typechecks against the kit: the SDK entries and the shared packages, and nothing else', () => {
    const cfg = kitTsconfig('/kit', '/app') as { compilerOptions: { paths: Record<string, string[]>; types: string[] }; include: string[] }
    expect(cfg.compilerOptions.paths['@talaria/sdk']).toEqual([`/kit/${KIT_PATHS.types.client}`])
    expect(cfg.compilerOptions.paths['@talaria/sdk/server']).toEqual([`/kit/${KIT_PATHS.types.server}`])
    expect(cfg.compilerOptions.paths['svelte/*']).toEqual(['/kit/node_modules/svelte/*'])
    // zod is not an app's to import (z comes from @talaria/sdk/server), and
    // there is no `@/`: a host internal must not typecheck here and fail there.
    expect(Object.keys(cfg.compilerOptions.paths)).not.toContain('zod')
    expect(Object.keys(cfg.compilerOptions.paths).some((k) => k.startsWith('@/'))).toBe(false)
    expect(Object.keys(cfg.compilerOptions.paths)).not.toContain('*')
    expect(cfg.include).toEqual(['/app/**/*.ts', '/app/**/*.svelte'])
  })

  it("runs the app's tests with the SDK entries mapped exactly, so '@talaria/sdk' never takes '/server'", () => {
    const src = kitVitestConfig('/kit', '/app', '/work')
    expect(src).toContain(`{ find: /^\\@talaria\\/sdk\\/server$/, replacement: "/kit/${KIT_PATHS.runtime.server}" }`)
    expect(src).toContain(`{ find: /^\\@talaria\\/sdk$/, replacement: "/kit/${KIT_PATHS.runtime.client}" }`)
    expect(src).toContain('"passWithNoTests":true')
  })
})

describe('eval transports', () => {
  it('serves recorded replies in order, says when a prompt moved, and asks for a re-record past the end', async () => {
    const drift: number[] = []
    const t = recordedTransport([{ text: 'one', prompt: promptDigest(req('a')) }, { text: 'two', prompt: 'stale' }], (n) => drift.push(n))
    expect((await t(req('a'))).text).toBe('one')
    expect((await t(req('b'))).text).toBe('two')
    expect(drift).toEqual([1])
    await expect(t(req('c'))).rejects.toThrow(/holds 2 replies for this fixture and the runner asked for another/)
  })

  it("builds the body the host's own gateway transport builds", () => {
    expect(gatewayBody(req('hi', { temperature: 0.2, effort: 'low' }))).toEqual({
      model: 'm',
      messages: [{ role: 'user', content: 'hi' }],
      stream: false,
      temperature: 0.2,
      reasoning_effort: 'low',
    })
    const tools = gatewayBody(req('hi', { effort: 'high', toolDefs: [{ name: 'f', description: 'd', parameters: { type: 'object' } }], jsonMode: true }))
    expect(tools).toMatchObject({ tool_choice: 'auto', reasoning_effort: 'none', response_format: { type: 'json_object' } })
    expect(tools.tools).toEqual([{ type: 'function', function: { name: 'f', description: 'd', parameters: { type: 'object' } } }])
  })

  it('calls the relay with the credential and the eval session, and reports its refusals by status', async () => {
    const seen: Array<{ url: string; headers: Record<string, string> }> = []
    const ok = (async (url: string, init: RequestInit) => {
      seen.push({ url, headers: init.headers as Record<string, string> })
      return new Response(JSON.stringify({ choices: [{ message: { content: '{"x":1}' } }], usage: { prompt_tokens: 7, completion_tokens: 3 } }))
    }) as unknown as typeof fetch
    const reply = await gatewayTransport({ url: 'http://app:5307/', key: 'tlk_wb', session: 'wbeval_ab' }, ok)(req('hi'))
    expect(reply).toMatchObject({ kind: 'gateway', text: '{"x":1}', usage: { promptTokens: 7, completionTokens: 3 } })
    expect(seen[0]!.url).toBe('http://app:5307/api/llm/v1/chat/completions')
    expect(seen[0]!.headers).toMatchObject({ authorization: 'Bearer tlk_wb', 'x-talaria-session': 'wbeval_ab' })
    const refused = (async () => new Response(JSON.stringify({ error: { message: 'invalid API key' } }), { status: 401 })) as unknown as typeof fetch
    await expect(gatewayTransport({ url: 'http://app', key: 'k' }, refused)(req('hi'))).rejects.toThrow('the gateway answered 401: invalid API key')
    await expect(gatewayTransport({ url: 'http://app', key: 'k' })(req('hi', { tools: 'own' }))).rejects.toThrow()
  })

  it("finds the job's eval session above the app, and only a well-formed one", () => {
    const job = join(root, 'job')
    const app = join(job, 'repo')
    mkdirSync(join(job, '.talaria'), { recursive: true })
    mkdirSync(app, { recursive: true })
    expect(findEvalSession(app)).toBeNull()
    writeFileSync(join(job, '.talaria', 'eval-session.json'), JSON.stringify({ key: 'wbeval_00', jobId: 'j-1' }))
    expect(findEvalSession(app)).toEqual({ key: 'wbeval_00', jobId: 'j-1' })
    writeFileSync(join(job, '.talaria', 'eval-session.json'), JSON.stringify({ key: 'has space' }))
    expect(findEvalSession(app)).toBeNull()
  })
})

describe('runEvals', () => {
  const TIDY = z.object({ title: z.string(), changed: z.boolean() })
  const harness = defineHarness<{ title: string }, z.infer<typeof TIDY>>({
    id: 'notes:tidy-title',
    label: 'Tidy',
    job: 'Tidies a title.',
    requires: ['json'],
    floor: { capabilities: [], refuseBelow: false, note: 'Needs JSON.' },
    model: {},
    render: (i) => [{ role: 'user', content: i.title }],
    output: { kind: 'json', schema: TIDY },
    onFailure: 'null',
    evals: [
      { name: 'short', input: { title: 'a long rambling title' }, check: (v) => (v.title.split(' ').length <= 3 ? null : 'too long') },
      { name: 'kept', input: { title: 'ok' }, check: (v) => (v.changed ? 'changed a good title' : null) },
    ],
  })
  const quiet: Partial<HarnessDeps> = {
    guardConfig: async () => ({ mode: 'off', checks: {}, minConfidence: 0.5, policedHosts: [], coach: false }),
    guardText: async () => [],
  }
  const files = async () => [{ file: 'tidy-title.ts', load: async () => ({ default: harness }) }]
  const appDir = () => join(root, 'eval-app')

  it('grades each fixture against its recorded replies, and names one never recorded', async () => {
    mkdirSync(join(appDir(), 'harnesses', 'recorded'), { recursive: true })
    writeFileSync(
      recordingPath(appDir(), 'tidy-title.ts'),
      JSON.stringify({ harness: 'notes:tidy-title', model: 'gpt-x', cases: { short: [{ text: '{"title":"Short title","changed":true}' }] } }),
    )
    const r = await runEvals({ slug: 'notes', appName: 'Notes', appDir: appDir(), files, deps: quiet })
    expect(r.cases.map((c) => [c.case, c.status])).toEqual([
      ['short', 'pass'],
      ['kept', 'unrecorded'],
    ])
    expect(r.ok).toBe(false)
    expect(r.cases[0]!.model).toBe('recorded:gpt-x')
    const text = formatEvalReport(r, appDir())
    expect(text).toContain('PASS     short (1 reply)')
    expect(text).toContain('MISSING  kept')
    expect(text).toContain('2 fixtures: 1 pass, 1 missing')
  })

  it('runs them through the gateway and records what it answered, keeping earlier recordings', async () => {
    const answers: Record<string, string> = { 'a long rambling title': '{"title":"Rambling title","changed":true}', ok: '{"title":"ok","changed":false}' }
    const fetchImpl = (async (_url: string, init: RequestInit) => {
      const body = JSON.parse(String(init.body)) as { messages: Array<{ content: string }> }
      const user = body.messages.at(-1)!.content
      const key = Object.keys(answers).find((k) => user.includes(k))!
      return new Response(JSON.stringify({ choices: [{ message: { content: answers[key] } }], usage: { prompt_tokens: 5, completion_tokens: 4 } }))
    }) as unknown as typeof fetch
    const r = await runEvals({
      slug: 'notes',
      appName: 'Notes',
      appDir: appDir(),
      files,
      deps: quiet,
      case: 'kept',
      gateway: { url: 'http://app', key: 'k', model: 'gpt-y', session: 'wbeval_1' },
      record: true,
      fetchImpl,
    })
    expect(r.cases.map((c) => [c.case, c.status, c.source, c.promptTokens])).toEqual([['kept', 'pass', 'gateway', 5]])
    expect(r.ok).toBe(true)
    const rec = readRecording(recordingPath(appDir(), 'tidy-title.ts'))
    if (!rec || typeof rec === 'string') throw new Error(String(rec))
    expect(Object.keys(rec.cases).sort()).toEqual(['kept', 'short'])
    expect(rec.cases.kept![0]!.text).toBe('{"title":"ok","changed":false}')
    expect(rec.cases.kept![0]!.prompt).toMatch(/^[0-9a-f]{16}$/)
    expect(rec.model).toBe('gpt-y')
    // Replayed offline, now both are there.
    const again = await runEvals({ slug: 'notes', appName: 'Notes', appDir: appDir(), files, deps: quiet })
    expect(again.cases.map((c) => [c.case, c.status])).toEqual([
      ['short', 'pass'],
      ['kept', 'pass'],
    ])
    expect(again.ok).toBe(true)
  })

  it("fails a fixture on its own check, and reports a reply that breaks the contract as an error", async () => {
    writeFileSync(
      recordingPath(appDir(), 'tidy-title.ts'),
      JSON.stringify({ harness: 'notes:tidy-title', cases: { short: [{ text: '{"title":"still a long rambling title","changed":true}' }], kept: [{ text: 'not json at all' }, { text: 'nor this' }] } }),
    )
    const r = await runEvals({ slug: 'notes', appName: 'Notes', appDir: appDir(), files, deps: quiet })
    expect(r.cases.map((c) => [c.case, c.status, c.reason])).toEqual([
      ['short', 'fail', 'too long'],
      ['kept', 'error', expect.any(String)],
    ])
    expect(r.ok).toBe(false)
  })
})

describe('the build report', () => {
  const base: BuildReport = {
    app: { name: 'Notes', slug: 'notes', dir: '/app' },
    kit: 'dev-1-2',
    build: { status: 'ready', key: 'abc', out: '/work/app-builds/notes/abc', files: ['app.js', 'app.css', 'server.js', 'mcp.js', 'harnesses.js'], bytes: 4096, error: null },
    client: { declared: ['work'], built: true },
    server: true,
    mcp: { tools: ['items_list'], leftOut: ['"bad" has no description'] },
    harnesses: { registered: ['notes:tidy-title'], skipped: ['harnesses/broken.ts: it failed to compile: …'], warnings: [] },
    triggers: null,
    probe: { ok: true },
    ok: true,
  }

  it('reads per surface and per harness, the skipped ones named as Manage → Apps names them', () => {
    const text = formatBuildReport(base, '/elsewhere')
    expect(text).toContain('build      ready · key abc · 5 files · 4.0 KB')
    expect(text).toContain('mcp        1 tool: items_list')
    expect(text).toContain('left out: "bad" has no description')
    expect(text).toContain('harnesses  registered: notes:tidy-title · 1 skipped')
    expect(text).toContain('skipped: harnesses/broken.ts: it failed to compile: …')
    expect(text).toContain('probe      ok')
  })

  it('names a harness that loads with a warning under the harness line', () => {
    const warned = { ...base, harnesses: { ...base.harnesses!, warnings: ['harnesses/tidy.ts: harness "notes:tidy-title" pins "mockep/mock-1", which names no platform agent'] } }
    expect(formatBuildReport(warned)).toContain('warning: harnesses/tidy.ts: harness "notes:tidy-title" pins "mockep/mock-1"')
  })

  it('lists each trigger with when it fires and whether server.ts answers it', () => {
    const triggers = triggerReport([{ id: 'nightly', cron: '0 3 * * *' }, { id: 'on-ticket', event: 'ticket.created' }], {
      default: { triggers: { nightly: () => {}, 'on-ticket': () => {} } },
    })
    expect(triggers).toEqual({
      declared: [
        { id: 'nightly', cron: '0 3 * * *', handler: true },
        { id: 'on-ticket', event: 'ticket.created', handler: true },
      ],
      unanswered: [],
      undeclared: [],
    })
    const text = formatBuildReport({ ...base, triggers })
    expect(text).toContain('triggers   2 triggers')
    expect(text).toContain('nightly · cron 0 3 * * * (UTC) · handler found')
    expect(text).toContain('on-ticket · on ticket.created · handler found')
  })

  it('reads a declared trigger with no handler as a failure, and says why the host would not', () => {
    // server.ts answers a typo of the declared id: one trigger nothing
    // answers, one handler nothing calls.
    const triggers = triggerReport([{ id: 'nightly', cron: '0 3 * * *' }], { default: { fetch: () => new Response(), triggers: { nighly: () => {} } } })
    expect(triggers?.unanswered).toEqual(['nightly'])
    expect(triggers?.undeclared).toEqual(['nighly'])
    const text = formatBuildReport({ ...base, triggers, ok: false })
    expect(text).toContain('triggers   1 trigger · 1 with no handler')
    expect(text).toContain('nightly · cron 0 3 * * * (UTC) · NO HANDLER — server.ts must answer it in defineAppServer({ triggers: { "nightly": … } })')
    expect(text).toContain('handler "nighly" answers no trigger in talaria.json')
    expect(text).toContain('records a failed run each time it fires')
    // No server at all answers nothing; a server with no triggers and none
    // declared has no line to report.
    expect(triggerReport([{ id: 'nightly', cron: '0 3 * * *' }], null)?.unanswered).toEqual(['nightly'])
    expect(triggerReport([], { default: { fetch: () => new Response() } })).toBeNull()
    // A non-function under the id is no handler — what the host's trigger route calls.
    expect(triggerReport([{ id: 'x', event: 'ticket.created' }], { default: { triggers: { x: 'nope' } } })?.unanswered).toEqual(['x'])
  })

  it('says a failed build or probe is what the host would refuse', () => {
    expect(formatBuildReport({ ...base, build: { ...base.build, status: 'failed', error: '"zod" is not shared with apps' }, ok: false })).toContain(
      'build      FAILED — "zod" is not shared with apps',
    )
    expect(formatBuildReport({ ...base, probe: { ok: false, error: 'server.ts must default-export defineAppServer({ fetch })' }, ok: false })).toContain(
      'probe      FAILED — server.ts must default-export',
    )
  })
})

describe('talaria-app', () => {
  const kit = () => {
    const dir = join(root, 'kit')
    mkdirSync(dir, { recursive: true })
    writeFileSync(join(dir, 'kit.json'), JSON.stringify({ format: KIT_FORMAT, version: 'dev-1-abcd', hostBuild: 'dev-1', sdk: '1.0.0' }))
    return dir
  }
  const run = async (argv: string[], cwd = root, env: NodeJS.ProcessEnv = {}) => {
    const out: string[] = []
    const err: string[] = []
    const code = await main(argv, { out: (s) => out.push(s), err: (s) => err.push(s), cwd, env, kitRoot: kit() })
    return { code, out: out.join('\n'), err: err.join('\n') }
  }

  it('prints its usage, and refuses a usage error with status 2', async () => {
    expect((await run([])).code).toBe(2)
    expect((await run(['--help'])).out).toContain('usage: talaria-app <command>')
    const bad = await run(['deploy'])
    expect(bad.code).toBe(2)
    expect(bad.err).toContain('unknown command deploy')
  })

  it('says which kit it is', async () => {
    expect((await run(['version'])).out).toBe('kit dev-1-abcd · @talaria/sdk 1.0.0 · host build dev-1')
  })

  it('refuses a directory that is not a kit', async () => {
    const r = await run(['version', '--kit', root])
    expect(r.code).toBe(2)
    expect(r.err).toContain('is not an SDK kit')
  })

  it('scaffolds into a directory, and finds no app where there is none', async () => {
    const dir = join(root, 'talaria-app-fresh')
    mkdirSync(dir, { recursive: true })
    const made = await run(['scaffold', '--name', 'Fresh'], dir)
    expect(made.code).toBe(0)
    expect(made.out).toContain('wrote 8 files')
    const none = await run(['build'], join(root, 'kit'))
    expect(none.code).toBe(1)
    expect(none.err).toContain('no talaria.json at or above')
  })

  it('asks for a gateway and a model before it calls one', async () => {
    const app = join(root, 'talaria-app-fresh')
    // No credentials: never reaches the build.
    const r = await run(['eval', '--record'], app)
    expect(r.code).toBe(2)
    expect(r.err).toContain('--record writes what the gateway answered, so it needs --gateway')
  })
})
