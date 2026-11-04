import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'
import { PLATFORM_AGENTS, type PlatformAgentId } from '@/server/platform-agents'

// THE TS TWIN OF THE RUST CATALOG. `PLATFORM_AGENTS` in
// api/crates/talaria-platform-agents/src/lib.rs is the list the admin panel
// serves and the Rust harnesses pin against; `PlatformAgentId` here is what a
// TS `ModelSpec.pin` (and so an app harness) may name. The Rust list grew
// 'ticket-relevance' and this one did not, so a pin the platform honours was a
// type error here. Read the Rust source rather than restate it, so the next
// agent added there fails this test until it is added here too.
const RUST = new URL('../../../api/crates/talaria-platform-agents/src/lib.rs', import.meta.url)

const rustCatalog = (): Array<{ id: string; assignable: boolean }> => {
  const src = readFileSync(RUST, 'utf8')
  const start = src.indexOf('pub const PLATFORM_AGENTS')
  expect(start).toBeGreaterThan(-1)
  const body = src.slice(start, src.indexOf('];', start))
  // One chunk per entry. Not a `[^}]*` scan: the job strings carry `\u{2019}`.
  return body
    .split(/\bPlatformAgent\s*\{/)
    .slice(1)
    .map((entry) => ({
      id: /\bid:\s*"([^"]+)"/.exec(entry)?.[1] ?? '',
      assignable: /\bassignable:\s*true\b/.test(entry),
    }))
}

describe('PLATFORM_AGENTS matches the Rust catalog', () => {
  it('names the same agents, in the same order, with the same assignability', () => {
    const rust = rustCatalog()
    expect(rust.length).toBeGreaterThan(0)
    expect(PLATFORM_AGENTS.map(({ id, assignable }) => ({ id, assignable }))).toEqual(rust)
  })

  it("types the ticket gate as a pin a harness may name", () => {
    const pin: PlatformAgentId = 'ticket-relevance'
    expect(PLATFORM_AGENTS.find((a) => a.id === pin)?.label).toBe('Ticket gate')
  })
})
