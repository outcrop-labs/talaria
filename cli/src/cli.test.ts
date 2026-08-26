// Router behavior: dispatch, help, flag forms, suggestions, aliases. All
// against synthetic trees — real commands test their own dispatch.

import { describe, expect, test } from 'bun:test'
import { dispatch, parseArgs, renderHelp, type Group, type Leaf } from './cli'
import { fakeCtx } from './testing'
import { CliError } from './ui'

const leaf = (name: string, over: Partial<Leaf> = {}): Leaf => ({
  kind: 'leaf',
  name,
  summary: `the ${name} command`,
  run: () => 0,
  ...over,
})

const tree = (): Group => ({
  kind: 'group',
  name: 'talaria',
  summary: 'test tree',
  children: [
    leaf('dev', { positionals: { name: 'extra', multiple: true } }),
    { kind: 'group', name: 'box', aliases: ['boxes'], summary: 'boxes', children: [leaf('new', { flags: [{ name: 'branch', kind: 'value', desc: 'branch' }], positionals: { name: 'name', required: true } })] },
  ],
})

describe('parseArgs', () => {
  const flags = [
    { name: 'branch', kind: 'value' as const, desc: 'b' },
    { name: 'force', kind: 'bool' as const, desc: 'f' },
    { name: 'keep', kind: 'value' as const, desc: 'k', default: '7' },
  ]

  test('space-separated and = forms are equivalent', () => {
    expect(parseArgs(['--branch', 'x'], flags, 't').flags.branch).toBe('x')
    expect(parseArgs(['--branch=y'], flags, 't').flags.branch).toBe('y')
  })

  test('bools set true; absent value flags take defaults', () => {
    const a = parseArgs(['--force'], flags, 't')
    expect(a.flags.force).toBe(true)
    expect(a.flags.keep).toBe('7')
  })

  test('unknown flag names the command', () => {
    expect(() => parseArgs(['--branh'], flags, 'talaria box new')).toThrow(
      'talaria box new: unknown flag --branh',
    )
  })

  test('dangling value flag errors rather than eating a positional', () => {
    expect(() => parseArgs(['--branch'], flags, 't')).toThrow('needs a value')
  })

  test('-- makes everything after it positional', () => {
    const a = parseArgs(['--', '--not-a-flag', 'x'], flags, 't')
    expect(a.positionals).toEqual(['--not-a-flag', 'x'])
  })
})

describe('dispatch', () => {
  test('unknown command suggests the nearest name', async () => {
    const ctx = fakeCtx()
    const code = await dispatch(ctx, tree(), ['box', 'nw', 'demo'])
    expect(code).toBe(1)
    const fail = ctx.logLines.find((l) => l.kind === 'fail')
    expect(fail?.msg).toContain('did you mean `new`')
  })

  test('--help at group level renders the subtree, exit 0', async () => {
    const ctx = fakeCtx()
    expect(await dispatch(ctx, tree(), ['box', '--help'])).toBe(0)
    const raw = ctx.logLines.find((l) => l.kind === 'raw')!
    expect(raw.msg).toContain('new')
    expect(raw.msg).not.toContain('talaria dev')
  })

  test('bare invocation prints the whole menu', async () => {
    const ctx = fakeCtx()
    expect(await dispatch(ctx, tree(), [])).toBe(0)
    expect(ctx.logLines.find((l) => l.kind === 'raw')!.msg).toContain('box')
  })

  test('aliases resolve; required positionals are enforced', async () => {
    const ctx = fakeCtx()
    expect(await dispatch(ctx, tree(), ['boxes', 'new', '--branch', 'b', 'demo'])).toBe(0)
    const code = await dispatch(ctx, tree(), ['box', 'new'])
    expect(code).toBe(1)
    expect(ctx.logLines.some((l) => l.kind === 'fail' && /is required/.test(l.msg))).toBe(true)
  })

  test('unexpected extra positional errors', async () => {
    const ctx = fakeCtx()
    // dev allows multiples; box new takes exactly one
    expect(await dispatch(ctx, tree(), ['box', 'new', 'a', 'b'])).toBe(1)
  })

  test('flag on a group explains itself', async () => {
    const ctx = fakeCtx()
    expect(await dispatch(ctx, tree(), ['box', '--force'])).toBe(1)
    expect(ctx.logLines.some((l) => l.kind === 'fail' && /command group/.test(l.msg))).toBe(true)
  })
})

describe('renderHelp', () => {
  test('leaf help lists flags with descriptions', () => {
    const h = renderHelp(['talaria', 'box', 'new'], {
      kind: 'leaf',
      name: 'new',
      summary: 'make a box',
      flags: [{ name: 'qdrant', kind: 'bool', desc: 'also seed qdrant' }],
      positionals: { name: 'name', required: true },
      run: () => 0,
    })
    expect(h).toContain('--qdrant')
    expect(h).toContain('also seed qdrant')
    expect(h).toContain('<name>')
  })
})
