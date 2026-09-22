import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import { EFFORTS, humanAssigneeId, isHumanAssignee, OFF_BOARD_STATUSES, PRIORITIES, STATUS_LABEL, TASK_STATUSES } from '@/lib/task-const'

describe('assignee encoding', () => {
  it('distinguishes a human from an agent model id', () => {
    expect(isHumanAssignee('user:3f2504e0-4f89-11d3-9a0c-0305e82c3301')).toBe(true)
    expect(isHumanAssignee('claude-opus-5')).toBe(false)
    expect(isHumanAssignee('')).toBe(false)
    // A model id that merely contains "user:" is not a human.
    expect(isHumanAssignee('agent/user:thing')).toBe(false)
  })

  it('strips the prefix to recover the user id', () => {
    expect(humanAssigneeId('user:3f2504e0-4f89-11d3-9a0c-0305e82c3301')).toBe('3f2504e0-4f89-11d3-9a0c-0305e82c3301')
  })
})

describe('constants', () => {
  it('has a label for every status, including the off-board ones', () => {
    // Imported, not spelled out: a copy of this list that drifts leaves tickets
    // in a status no view draws. check-invariants.mjs enforces that, and caught
    // this very line when the test was ported.
    for (const s of [...TASK_STATUSES, ...OFF_BOARD_STATUSES]) expect(STATUS_LABEL[s]).toBeTruthy()
  })

  it('keeps the ordered scales ordered', () => {
    expect(PRIORITIES).toEqual(['low', 'medium', 'high', 'urgent'])
    expect(EFFORTS).toEqual(['xs', 's', 'm', 'l', 'xl'])
  })
})

describe('off-board pin', () => {
  // No import crosses languages, so the pin reads the Rust engine's source and
  // compares (same move as the secretbox fixtures test reading api/tests/). If
  // the Rust literal is renamed or reflows so the regex misses, this parses to
  // [] and FAILS against the client list — it cannot silently pass.
  const CRATES = join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..', 'api', 'crates')
  const RUST_STATUSES = join(CRATES, 'talaria-statuses', 'src', 'lib.rs')
  const RUST_TASK_CONST = join(CRATES, 'talaria-task-const', 'src', 'lib.rs')

  it('matches the Rust statuses engine list exactly', () => {
    const src = readFileSync(RUST_STATUSES, 'utf8')
    const literal = src.match(/pub const OFF_BOARD_STATUSES: &\[&str\] = &\[([^\]]*)\]/)?.[1] ?? ''
    const rust = literal.match(/"([^"]+)"/g)?.map((s) => s.slice(1, -1)) ?? []
    expect(rust).toEqual([...OFF_BOARD_STATUSES])
  })

  // The same pin, for the two ladders this file's client list feeds the
  // composer with: the Rust crate the api validates against and the client
  // must offer the same words in the same order, or a ticket picks a priority
  // no engine accepts.
  const rustList = (file: string, name: string): string[] => {
    const src = readFileSync(file, 'utf8')
    const literal = src.match(new RegExp(`pub const ${name}: &\\[&str\\] = &\\[([^\\]]*)\\]`))?.[1] ?? ''
    return literal.match(/"([^"]+)"/g)?.map((s) => s.slice(1, -1)) ?? []
  }

  it('matches talaria-task-const on exactly the priority ladder', () => {
    expect(rustList(RUST_TASK_CONST, 'PRIORITIES')).toEqual([...PRIORITIES])
  })

  it('matches talaria-task-const on exactly the effort ladder', () => {
    expect(rustList(RUST_TASK_CONST, 'EFFORTS')).toEqual([...EFFORTS])
  })

  // mcp/src/index.ts is the agent-facing surface: it documents the statuses an
  // agent may set. It must not quietly grow one the engine refuses.
  it("matches mcp's AGENT_STATUSES on the statuses an agent may set", () => {
    const src = readFileSync(join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..', 'mcp', 'src', 'index.ts'), 'utf8')
    const literal = src.match(/AGENT_STATUSES = \[([^\]]*)\]/)?.[1] ?? ''
    const mcp = literal.match(/'([^']+)'/g)?.map((s) => s.slice(1, -1)) ?? []
    expect(mcp.length).toBeGreaterThan(0)
    expect(mcp).toEqual(['in_progress', 'blocked', 'quality_review'])
  })
})
