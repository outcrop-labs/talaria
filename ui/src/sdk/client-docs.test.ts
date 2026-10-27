import { spawnSync } from 'node:child_process'
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { compile } from 'svelte/compiler'
import { afterAll, describe, expect, it } from 'vitest'
import { harnessStarterFiles, skeletonFiles } from '../../../cli/src/cmd/app/skeleton'

// THE CLIENT DOCS AND THE SCAFFOLD, COMPILED AGAINST THE SDK.
//
// server.test.ts does this for the harness docs. The client side had nothing:
// the docs typed /api/agents with a field it does not return, showed a
// `<Waiting>` without its required label, and paired invalidate('things')
// with a query it never matched — and the scaffold every new app starts from
// rendered "No items yet" over a failed read. A document is not a build
// target, so each of those shipped and stayed.
//
// So every `svelte` fence in client.md, ui-kit.md, getting-started.md, mcp.md
// and the quick guide in apps/README.md, every `ts` fence there that imports
// @talaria/sdk, @talaria/sdk/server or @talaria/sdk/testing, and every file
// `talaria app new --harness` (and the kit's `talaria-app scaffold`) writes
// are copied out VERBATIM and run through two passes:
// the Svelte compiler (syntax, runes), then svelte-check against the real SDK
// under the ui's own tsconfig (types, props, unused imports). A rename in the
// SDK, or a doc edit that stops compiling, fails here.
//
// A fence whose first line names its file (`<!-- apps/<slug>/Work.svelte -->`,
// `// apps/<slug>/app.ts`) is written under that name, so the doc's own
// imports between fences resolve; a sibling `.svelte` a fence imports that no
// fence provides gets a one-line stub.

const UI = fileURLToPath(new URL('../../', import.meta.url))
const REPO = join(UI, '..')
// Repo-relative. apps/README.md is the guide an author meets first, beside
// their own app, so its examples are held to the same bar.
const DOC_FILES = ['docs/sdk/client.md', 'docs/sdk/ui-kit.md', 'docs/sdk/getting-started.md', 'docs/sdk/mcp.md', 'apps/README.md']

interface Example {
  /** Where it came from, for the failure message. */
  origin: string
  /** Path inside the scratch workspace. */
  path: string
  code: string
}

function fences(doc: string): Example[] {
  const lines = readFileSync(join(REPO, doc), 'utf8').split('\n')
  const out: Example[] = []
  const base = doc.replace(/\.md$/, '')
  for (let i = 0; i < lines.length; i++) {
    const open = /^```(svelte|ts)\s*$/.exec(lines[i] ?? '')
    if (!open) continue
    const start = i + 1
    let end = start
    while (end < lines.length && !/^```\s*$/.test(lines[end] ?? '')) end++
    const code = lines.slice(start, end).join('\n') + '\n'
    i = end
    const lang = open[1]
    if (lang === 'ts' && !/from '@talaria\/sdk(?:\/server|\/testing)?'/.test(code)) continue
    const named = /^(?:<!--|\/\/)\s*apps\/<slug>\/([\w.-]+\.(?:svelte|ts))/.exec(code)?.[1]
    const file = named ?? `example-${start}.${lang === 'svelte' ? 'svelte' : 'ts'}`
    out.push({ origin: `${doc}:${start}`, path: `${base}/${file}`, code })
  }
  return out
}

function examples(): Example[] {
  const docs = DOC_FILES.flatMap(fences)
  const scaffold = Object.entries({ ...skeletonFiles({ name: 'Doc Check', icon: '◇' }), ...harnessStarterFiles({ name: 'Doc Check' }) })
    .filter(([file]) => /\.(svelte|ts)$/.test(file))
    .map(([file, code]) => ({ origin: `cli/src/cmd/app/skeleton.ts → ${file}`, path: `skeleton/${file}`, code }))
  return [...docs, ...scaffold]
}

const all = examples()
let workspace: string | null = null

afterAll(() => {
  if (workspace) rmSync(workspace, { recursive: true, force: true })
})

describe('the client docs and the scaffold', () => {
  it('has the examples it claims to check', () => {
    // A doc reshuffle that silently drops every fence would otherwise pass.
    for (const doc of DOC_FILES) expect(all.some((e) => e.origin.startsWith(`${doc}:`)), doc).toBe(true)
    expect(all.map((e) => e.path)).toEqual(
      expect.arrayContaining(['skeleton/Work.svelte', 'skeleton/app.ts', 'skeleton/harnesses/tidy-title.ts', 'docs/sdk/getting-started/Work.svelte', 'apps/README/Work.svelte']),
    )
  })

  it('every Svelte example compiles', () => {
    const broken: string[] = []
    for (const e of all.filter((x) => x.path.endsWith('.svelte'))) {
      try {
        compile(e.code, { filename: e.path, generate: 'client', runes: true })
      } catch (err) {
        broken.push(`${e.origin}: ${err instanceof Error ? err.message : String(err)}`)
      }
    }
    expect(broken).toEqual([])
  })

  it('every example typechecks against @talaria/sdk', () => {
    // Inside ui/ so `@talaria/sdk`, `@/…` and `svelte` resolve exactly as they
    // do for an installed app; removed in afterAll.
    workspace = mkdtempSync(join(UI, '.sdk-doc-examples-'))
    const origins = new Map<string, string>()
    for (const e of all) {
      const abs = join(workspace, e.path)
      mkdirSync(dirname(abs), { recursive: true })
      writeFileSync(abs, e.code)
      origins.set(e.path, e.origin)
    }
    for (const e of all) {
      for (const m of e.code.matchAll(/from '\.\/([\w-]+\.svelte)'/g)) {
        const sibling = join(workspace, dirname(e.path), m[1]!)
        // lang="ts": a script-less component is a JS module, which the
        // strict tsconfig refuses to type (implicitly any).
        if (!existsSync(sibling)) writeFileSync(sibling, '<script lang="ts"></script>\n\n<p>stub</p>\n')
      }
    }
    writeFileSync(
      join(workspace, 'tsconfig.json'),
      JSON.stringify({ extends: '../tsconfig.json', include: ['./**/*.ts', './**/*.svelte'], exclude: [] }),
    )

    const require = createRequire(import.meta.url)
    const bin = join(dirname(require.resolve('svelte-check/package.json')), 'bin/svelte-check')
    const run = spawnSync(
      process.execPath,
      [bin, '--tsconfig', join(workspace, 'tsconfig.json'), '--workspace', workspace, '--output', 'machine', '--threshold', 'error'],
      { cwd: UI, encoding: 'utf8', timeout: 280_000, maxBuffer: 16 * 1024 * 1024 },
    )
    const output = `${run.stdout ?? ''}${run.stderr ?? ''}`
    expect(output, 'svelte-check did not finish').toMatch(/COMPLETED/)
    const errors = output
      .split('\n')
      .filter((l) => / ERROR /.test(l))
      .map((l) => {
        const m = /ERROR "([^"]+)" (\d+):(\d+) "(.*)"$/.exec(l)
        if (!m) return l
        const [, file, line, col, message] = m
        return `${origins.get(file!) ?? file} (${file}:${line}:${col}): ${message}`
      })
    expect(errors).toEqual([])
  }, 300_000)
})
