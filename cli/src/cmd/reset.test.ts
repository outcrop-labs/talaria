// reset.sh's safety rails, as tests: the localhost-only guard (BEFORE
// anything), the typed confirm (wrong word = zero destructive execs), and the
// fleet teardown scope. `die` throws CliError, so refusals assert on the
// throw; the counts/warnings print through the planted log.

import { describe, expect, test } from 'bun:test'
import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { isLocalDb, runReset } from './reset'
import { fakeCtx, type FakeCtx } from '../testing'
import { CliError } from '../ui'

const attempt = async (fn: () => Promise<unknown>): Promise<string> => {
  try {
    await fn()
    return ''
  } catch (e) {
    return e instanceof CliError ? e.message : `<unexpected throw: ${String(e)}>`
  }
}

const makeTree = (dbUrl: string): string => {
  const root = mkdtempSync(join(tmpdir(), 'talaria-reset-'))
  mkdirSync(join(root, 'ui'), { recursive: true })
  writeFileSync(join(root, 'ui/.env'), `DATABASE_URL=${dbUrl}\n`)
  return root
}

const plantUp = (ctx: FakeCtx): void => {
  ctx.plant(['docker', ['inspect', '-f', '{{.State.Running}}', 'talaria-postgres-dev']], 'true\n')
}

describe('reset — localhost guard', () => {
  test('isLocalDb accepts loopback spellings only', () => {
    expect(isLocalDb('postgres://talaria:talaria@127.0.0.1:5544/talaria')).toBe(true)
    expect(isLocalDb('postgres://talaria:talaria@localhost:5432/talaria')).toBe(true)
    expect(isLocalDb('postgres://talaria:talaria@prod.example.com:5432/talaria')).toBe(false)
    expect(isLocalDb('postgres://talaria:talaria@10.0.0.4:5432/talaria')).toBe(false)
  })

  test('a remote DATABASE_URL dies before a single exec runs', async () => {
    const ctx = fakeCtx()
    ctx.root = makeTree('postgres://u:p@prod.example.com:5432/talaria')
    const msg = await attempt(() => runReset(ctx, 'database'))
    expect(msg).toContain('does not point at localhost')
    expect(ctx.calls.length).toBe(0)
  })

  test('missing ui/.env dies pointing at setup', async () => {
    const ctx = fakeCtx()
    ctx.root = mkdtempSync(join(tmpdir(), 'talaria-empty-'))
    const msg = await attempt(() => runReset(ctx, 'secrets'))
    expect(msg).toContain('bun talaria setup')
  })

  test('postgres down dies before the counts', async () => {
    const ctx = fakeCtx()
    ctx.root = makeTree('postgres://t:t@127.0.0.1:5544/talaria')
    const msg = await attempt(() => runReset(ctx, 'secrets'))
    expect(msg).toContain('is not running')
  })
})

describe('reset — typed confirm', () => {
  test('wrong word aborts with zero destructive execs recorded', async () => {
    const ctx = fakeCtx({ reply: 'secrets ' }) // trailing space: trimmed by a shell's `read`, and by us
    ctx.root = makeTree('postgres://t:t@127.0.0.1:5544/talaria')
    plantUp(ctx)
    // counts all plant to 0 by default (unplanted exec → empty stdout)
    const msg = await attempt(() => runReset(ctx, 'database'))
    expect(msg).toBe('aborted — nothing was changed')
    const destructive = ctx.calls.filter((c) =>
      c.args.includes('ON_ERROR_STOP=1') || (c.args[0] === 'rm' && c.args[1] === '-f') || c.args.includes('down'))
    expect(destructive).toHaveLength(0)
  })

  test('the right word (after trim) proceeds to the batch', async () => {
    const ctx = fakeCtx({ reply: 'database' })
    ctx.root = makeTree('postgres://t:t@127.0.0.1:5544/talaria')
    plantUp(ctx)
    await runReset(ctx, 'database')
    const batch = ctx.calls.find((c) => c.args.includes('ON_ERROR_STOP=1'))!
    expect(batch.args.join(' ')).toContain('drop database if exists talaria')
    expect(batch.args.join(' ')).toContain('-d postgres')
  })
})

describe('reset — secrets scope', () => {
  test('clears ciphertext first, keys last, in one batch', async () => {
    const ctx = fakeCtx({ reply: 'secrets' })
    ctx.root = makeTree('postgres://t:t@127.0.0.1:5544/talaria')
    plantUp(ctx)
    await runReset(ctx, 'secrets')
    const batch = ctx.calls.find((c) => c.args.includes('ON_ERROR_STOP=1'))!
    const sql = batch.args[batch.args.indexOf('-c') + 1]!
    expect(sql).toContain('update llm_endpoints set api_key_cipher = null')
    expect(sql.indexOf('update llm_endpoints')).toBeLessThan(sql.indexOf('delete from secret_keys'))
    expect(sql.startsWith('begin;')).toBe(true)
    expect(sql.endsWith('commit;')).toBe(true)
  })
})

describe('reset — fleet scope', () => {
  test('removes rendered artifacts, keeps fleet/chassis.yml + skills', async () => {
    const root = makeTree('postgres://t:t@127.0.0.1:5544/talaria')
    mkdirSync(join(root, 'fleet/agents/x'), { recursive: true })
    writeFileSync(join(root, 'fleet/chassis.yml'), 'network:\n  name: talaria\n')
    writeFileSync(join(root, 'fleet/.env'), 'LLM_API_KEY=x\n')
    const ctx = fakeCtx({ reply: 'fleet' })
    ctx.root = root
    plantUp(ctx)
    await runReset(ctx, 'fleet')
    expect(ctx.logLines.some((l) => l.msg.includes('agent_defs in the database are KEPT'))).toBe(true)
    // rendered artifacts gone; user-owned config plane intact
    expect(await Bun.file(join(root, 'fleet/.env')).exists()).toBe(false)
    expect(await Bun.file(join(root, 'fleet/agents/x')).exists()).toBe(false)
    expect(await Bun.file(join(root, 'fleet/chassis.yml')).exists()).toBe(true)
  })

  test('leftover agent containers are force-removed', async () => {
    const ctx = fakeCtx({ reply: 'fleet' })
    ctx.root = makeTree('postgres://t:t@127.0.0.1:5544/talaria')
    plantUp(ctx)
    ctx.plant(['docker', ['ps', '-a', '--format', '{{.Names}}']], 'talaria-fleet-agent-a\ntalaria-fleet-agent-b\ntalaria-postgres-dev\n')
    await runReset(ctx, 'fleet')
    expect(ctx.calls.some((c) => c.args[0] === 'rm' && c.args.includes('talaria-fleet-agent-a'))).toBe(true)
    expect(ctx.calls.some((c) => c.args[0] === 'rm' && c.args.includes('talaria-fleet-agent-b'))).toBe(true)
    expect(ctx.calls.some((c) => c.args[0] === 'rm' && c.args.includes('talaria-postgres-dev'))).toBe(false)
  })
})
