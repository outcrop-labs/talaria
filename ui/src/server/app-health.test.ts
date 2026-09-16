import { describe, expect, it } from 'vitest'
import { clientArtifactOk, moduleHasFetch, moduleHasMcpTools, moduleHasSurfaces } from './app-health'

describe('moduleHasFetch', () => {
  it('accepts defineAppServer shape', () => {
    expect(moduleHasFetch({ default: { fetch: async () => new Response() } })).toBe(true)
  })
  it('rejects a missing or non-function fetch', () => {
    expect(moduleHasFetch(null)).toBe(false)
    expect(moduleHasFetch({ default: {} })).toBe(false)
    expect(moduleHasFetch({ default: { fetch: 1 } })).toBe(false)
  })
})

describe('moduleHasMcpTools', () => {
  it('accepts defineAppMcp shape', () => {
    expect(moduleHasMcpTools({ default: { tools: [] } })).toBe(true)
  })
  it('rejects a module with no tools array', () => {
    expect(moduleHasMcpTools({ default: {} })).toBe(false)
    expect(moduleHasMcpTools({ tools: [] })).toBe(false)
  })
})

describe('moduleHasSurfaces', () => {
  it('accepts defineApp with at least one surface', () => {
    expect(moduleHasSurfaces({ default: { work: {} } })).toBe(true)
    expect(moduleHasSurfaces({ default: { manage: {}, settings: {} } })).toBe(true)
  })
  it('rejects an empty or missing default', () => {
    expect(moduleHasSurfaces(null)).toBe(false)
    expect(moduleHasSurfaces({ default: {} })).toBe(false)
    expect(moduleHasSurfaces({ work: {} })).toBe(false)
  })
})

describe('clientArtifactOk', () => {
  it('accepts an ESM bundle with an export', () => {
    expect(clientArtifactOk('const x = 1;\nexport { x as default };\n')).toBe(true)
  })
  it('rejects empty or non-module output', () => {
    expect(clientArtifactOk('')).toBe(false)
    expect(clientArtifactOk('(function(){var app=1})();')).toBe(false)
  })
})

