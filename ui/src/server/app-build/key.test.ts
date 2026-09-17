import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { sourceKey } from './key'


describe('sourceKey', () => {
  it('is stable for the same sources and host', () => {
    const dir = mkdtempSync(join(tmpdir(), 'talaria-app-key-'))
    writeFileSync(join(dir, 'app.ts'), 'export default {}\n')
    const a = sourceKey(dir, 'host-1')
    const b = sourceKey(dir, 'host-1')
    expect(a).toBe(b)
    expect(a).toMatch(/^[0-9a-f]{16}$/)
  })

  it('changes when a source file changes', () => {
    const dir = mkdtempSync(join(tmpdir(), 'talaria-app-key-'))
    writeFileSync(join(dir, 'app.ts'), 'export default {}\n')
    const before = sourceKey(dir, 'host-1')
    writeFileSync(join(dir, 'app.ts'), 'export default { work: true }\n')
    expect(sourceKey(dir, 'host-1')).not.toBe(before)
  })

  it('changes when the host build id changes', () => {
    const dir = mkdtempSync(join(tmpdir(), 'talaria-app-key-'))
    mkdirSync(join(dir, 'nested'))
    writeFileSync(join(dir, 'app.ts'), 'export default {}\n')
    expect(sourceKey(dir, 'host-a')).not.toBe(sourceKey(dir, 'host-b'))
  })
})
