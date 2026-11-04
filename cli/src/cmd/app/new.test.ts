import { describe, expect, test } from 'bun:test'
import { existsSync, mkdirSync, mkdtempSync, readFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { CliError } from '../../ui'
import { fakeCtx } from '../../testing'
import { RESERVED_APP_SLUGS, displayName, harnessFamily, harnessStarterFiles, skeletonFiles, standaloneRepoFiles } from './skeleton'
import { runAppNew } from './new'

const attempt = (fn: () => unknown): string => {
  try {
    fn()
    return ''
  } catch (e) {
    return e instanceof CliError ? e.message : `<unexpected throw: ${String(e)}>`
  }
}

const ctxAt = () => {
  const ctx = fakeCtx()
  ctx.root = mkdtempSync(join(tmpdir(), 'talaria-app-new-'))
  return ctx
}

describe('displayName', () => {
  test('title-cases hyphenated slugs', () => {
    expect(displayName('my-app')).toBe('My App')
    expect(displayName('crm')).toBe('Crm')
  })
})

describe('skeletonFiles', () => {
  test('embeds no slug anywhere: the installing instance decides it', () => {
    const files = skeletonFiles({ name: 'My App', icon: '◈' })
    expect(Object.keys(files).sort()).toEqual(['Item.svelte', 'Work.svelte', 'app.ts', 'mcp.ts', 'server.ts', 'talaria.json'])
    expect(JSON.parse(files['talaria.json']!)).toEqual({
      name: 'My App',
      icon: '◈',
      version: '0.1.0',
      description: 'My App — a Talaria app.',
      surfaces: { work: 'My App' },
    })
    for (const [path, body] of Object.entries(files)) {
      expect(body, path).not.toContain('my-app')
      expect(body, path).not.toMatch(/\bslug\b\s*=/)
    }
    expect(files['Work.svelte']).toContain('useAppApi()')
    expect(files['Work.svelte']).toContain('useAppInvalidate()')
    expect(files['Work.svelte']).toContain("useAppQuery<{ items: Item[] }>('items')")
    expect(files['Work.svelte']).toContain('{app.manifest.name}')
    expect(files['server.ts']).toContain("from '@talaria/sdk/server'")
    expect(files['mcp.ts']).toContain('items_list')
  })

  test('a failed read renders as a failure, never as "No items yet"', () => {
    const work = skeletonFiles({ name: 'My App', icon: '◈' })['Work.svelte']!
    // The empty state sits inside QueryState's `empty` snippet, which renders
    // only once the read RESOLVED empty; a failure takes QueryState's error branch.
    expect(work).toContain('<QueryState {query}')
    const empty = work.indexOf('{#snippet empty()}')
    expect(empty).toBeGreaterThan(work.indexOf('<QueryState'))
    expect(work.indexOf('title="No items yet"')).toBeGreaterThan(empty)
    expect(work).not.toContain('query.isLoading')
  })

  test('an item has an address of its own below the surface', () => {
    const files = skeletonFiles({ name: 'My App', icon: '◈' })
    const work = files['Work.svelte']!
    // /x/<slug>/items/<id>: the list links each item there, and the surface
    // routes off the address — never off component state a reload would lose.
    expect(work).toContain("route.match('items/:id')")
    expect(work).toContain('href={appHref(`items/${item.id}`)}')
    expect(work).toContain('<ItemPage id={open.id} />')
    expect(files['Item.svelte']).toContain('href={appHref()}')
    expect(files['server.ts']).toContain("method === 'GET'")
    expect(files['server.ts']).toContain("store.get('items', id)")
  })

  test('every write catches and says what failed', () => {
    const work = skeletonFiles({ name: 'My App', icon: '◈' })['Work.svelte']!
    expect(work.match(/catch \(e\) \{\n\s+await alert\(\{ title: 'Could not [a-z ]+', message: errorMessage\(e\) \}\)/g)?.length).toBe(2)
  })
})

describe('the starter harness and the repo files', () => {
  test("names the harness for the app, and records a reply its fixture passes on", () => {
    expect(harnessFamily('My App')).toBe('my-app')
    expect(harnessFamily('  Ünïcode & Co! ')).toBe('n-code-co')
    expect(harnessFamily('!!!')).toBe('app')
    const files = harnessStarterFiles({ name: 'Field Notes' })
    expect(Object.keys(files).sort()).toEqual(['harnesses/recorded/tidy-title.json', 'harnesses/tidy-title.ts'])
    const harness = files['harnesses/tidy-title.ts']!
    expect(harness).toContain('id: "field-notes:tidy-title"')
    expect(harness).toContain("from '@talaria/sdk/server'")
    // zod comes through the SDK: an app has no node_modules of its own.
    expect(harness).not.toContain("from 'zod'")
    const recorded = JSON.parse(files['harnesses/recorded/tidy-title.json']!) as { harness: string; cases: Record<string, Array<{ text: string }>> }
    expect(recorded.harness).toBe('field-notes:tidy-title')
    const fixture = /name: "([^"]+)"/.exec(harness)?.[1]
    expect(fixture).toBeDefined()
    const reply = JSON.parse(recorded.cases[fixture!]![0]!.text) as { title: string; changed: boolean }
    expect(reply.title.trim().split(/\s+/).length).toBeLessThanOrEqual(8)
  })

  test('a plugin repo ignores the kit\'s work and says how it is checked', () => {
    const files = standaloneRepoFiles({ name: 'Field Notes' })
    expect(files['.gitignore']).toContain('.talaria/')
    expect(files['README.md']).toContain('# Field Notes')
    expect(files['README.md']).toContain('talaria-app check')
  })
})

describe('runAppNew', () => {
  test('bad slug dies before writing', () => {
    const ctx = ctxAt()
    expect(attempt(() => runAppNew(ctx, 'MyApp'))).toContain('not a usable app slug')
    expect(existsSync(join(ctx.root, 'apps'))).toBe(false)
  })

  test('refuses a slug the router reserves, as install does', () => {
    // The same list Rust's install refuses: the shared fixture's.
    const fixture = JSON.parse(
      readFileSync(join(import.meta.dir, '..', '..', '..', '..', 'api', 'tests', 'it', 'fixtures', 'app-grants.json'), 'utf8'),
    ) as { routes: { reservedSlugs: string[] } }
    expect([...RESERVED_APP_SLUGS]).toEqual(fixture.routes.reservedSlugs)
    for (const slug of RESERVED_APP_SLUGS) {
      const ctx = ctxAt()
      expect(attempt(() => runAppNew(ctx, slug))).toContain('is reserved')
      expect(existsSync(join(ctx.root, 'apps'))).toBe(false)
    }
    expect(runAppNew(ctxAt(), 'hooks-board')).toBe(0)
  })

  test('refuses an existing destination', () => {
    const ctx = ctxAt()
    mkdirSync(join(ctx.root, 'apps', 'taken'), { recursive: true })
    expect(attempt(() => runAppNew(ctx, 'taken'))).toContain('already exists')
  })

  test('writes the skeleton under apps/<slug>', () => {
    const ctx = ctxAt()
    expect(runAppNew(ctx, 'pulse', { icon: '◎' })).toBe(0)
    const dir = join(ctx.root, 'apps', 'pulse')
    expect(JSON.parse(readFileSync(join(dir, 'talaria.json'), 'utf8')).icon).toBe('◎')
    expect(readFileSync(join(dir, 'app.ts'), 'utf8')).toContain('defineApp')
    expect(readFileSync(join(dir, 'Work.svelte'), 'utf8')).toContain('useAppQuery')
    expect(readFileSync(join(dir, 'Work.svelte'), 'utf8')).not.toContain('pulse')
    expect(ctx.logLines.some((l) => l.kind === 'say' && l.msg.includes('apps/pulse'))).toBe(true)
  })

  test('--harness adds the starter harness and its recorded reply', () => {
    const ctx = ctxAt()
    expect(runAppNew(ctx, 'notes', { harness: true })).toBe(0)
    const dir = join(ctx.root, 'apps', 'notes')
    expect(readFileSync(join(dir, 'harnesses', 'tidy-title.ts'), 'utf8')).toContain('"notes:tidy-title"')
    expect(existsSync(join(dir, 'harnesses', 'recorded', 'tidy-title.json'))).toBe(true)
    expect(existsSync(join(ctxAt().root, 'apps', 'notes'))).toBe(false)
  })

  test('TALARIA_APPS_DIR relocates the write', () => {
    const ctx = ctxAt()
    const custom = mkdtempSync(join(tmpdir(), 'talaria-apps-dir-'))
    ctx.env.TALARIA_APPS_DIR = custom
    expect(runAppNew(ctx, 'x')).toBe(0)
    expect(existsSync(join(custom, 'x', 'talaria.json'))).toBe(true)
    expect(existsSync(join(ctx.root, 'apps'))).toBe(false)
  })
})
