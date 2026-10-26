// The manifest rules across the two runtimes — the TS side of the tripwire.
// api/tests/it/fixtures/app-manifest.json is the one artifact both sides
// assert (Rust: talaria-app-manifest's tests), and docs/sdk/talaria.schema.json
// is held to the same rules here, so the published shape cannot drift from
// the validator that runs.
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it } from 'vitest'
import {
  KNOWN_KEYS,
  LIMITS,
  RESERVED_APP_SLUGS,
  SDK_VERSION,
  SLUG_RE,
  SURFACES,
  parseManifest,
  satisfiesRange,
  validateManifest,
} from './manifest'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..', '..')
const fixture = JSON.parse(readFileSync(join(ROOT, 'api/tests/it/fixtures/app-manifest.json'), 'utf8')) as {
  sdkVersion: string
  manifests: Array<{
    about: string
    manifest: unknown
    error?: string
    parsed?: { name: string; slug?: string; icon: string; version: string; surfaces: string[] }
  }>
  ranges: Array<{ range: string; version: string; satisfied?: boolean; error?: string }>
}
interface StringSchema {
  type?: string
  pattern?: string
  minLength?: number
  maxLength?: number
}
const schema = JSON.parse(readFileSync(join(ROOT, 'docs/sdk/talaria.schema.json'), 'utf8')) as {
  required: string[]
  properties: Record<
    string,
    {
      pattern?: string
      not?: { enum: string[] }
      maxLength?: number
      properties?: Record<string, StringSchema>
      additionalProperties?: boolean
      items?: { type: string; required: string[]; properties: Record<string, StringSchema>; oneOf: Array<{ required: string[] }> }
    }
  >
  'x-talaria-sdk-version': string
}

/** Whether the schema's `triggers.items` admits `item`, read for exactly the
 *  keywords it uses (type, required, properties' type/minLength/pattern,
 *  oneOf of required), as a JSON Schema validator would. */
function schemaAdmitsTrigger(item: unknown): boolean {
  const s = schema.properties.triggers?.items
  if (!s) throw new Error('the schema lost triggers.items')
  if (s.type !== 'object' || typeof item !== 'object' || item === null || Array.isArray(item)) return false
  const obj = item as Record<string, unknown>
  const has = (k: string) => Object.hasOwn(obj, k)
  if (!s.required.every(has)) return false
  for (const [k, p] of Object.entries(s.properties)) {
    if (!has(k)) continue
    const v = obj[k]
    if (p.type === 'string' && typeof v !== 'string') return false
    if (typeof v !== 'string') continue
    if (p.minLength !== undefined && [...v].length < p.minLength) return false
    if (p.pattern !== undefined && !new RegExp(p.pattern, 'u').test(v)) return false
  }
  return s.oneOf.filter((alt) => alt.required.every(has)).length === 1
}
const grants = JSON.parse(readFileSync(join(ROOT, 'api/tests/it/fixtures/app-grants.json'), 'utf8')) as {
  routes: { reservedSlugs: string[] }
}

describe('the shared manifest fixture', () => {
  it('names the SDK version this host provides', () => {
    expect(SDK_VERSION).toBe(fixture.sdkVersion)
  })

  for (const c of fixture.manifests) {
    it(c.about, () => {
      const got = validateManifest(c.manifest)
      if (c.error) {
        expect(got.ok).toBe(false)
        if (!got.ok) expect(got.error).toContain(c.error)
        return
      }
      expect(got).toMatchObject({ ok: true })
      if (got.ok && c.parsed) {
        expect(got.manifest.name).toBe(c.parsed.name)
        expect(got.manifest.icon).toBe(c.parsed.icon)
        expect(got.manifest.version).toBe(c.parsed.version)
        expect(got.manifest.slug).toBe(c.parsed.slug)
        expect(SURFACES.filter((k) => got.manifest.surfaces[k] !== undefined)).toEqual(c.parsed.surfaces)
      }
    })
  }

  for (const c of fixture.ranges) {
    it(`range ${JSON.stringify(c.range)} @ ${c.version}`, () => {
      const got = satisfiesRange(c.range, c.version)
      if (c.error) {
        expect(got.ok).toBe(false)
        if (!got.ok) expect(got.error).toContain(c.error)
      } else {
        expect(got).toEqual({ ok: true, satisfied: c.satisfied })
      }
    })
  }
})

describe('the file reader', () => {
  const enc = (s: string) => new TextEncoder().encode(s)
  it('refuses what Rust refuses', () => {
    expect(parseManifest(new Uint8Array([0x7b, 0x22, 0x6e, 0x22, 0x3a, 0x22, 0xe9, 0x22, 0x7d]))).toEqual({ ok: false, error: 'talaria.json is not UTF-8' })
    const bom = parseManifest(new Uint8Array([0xef, 0xbb, 0xbf, ...enc('{"name":"Bom"}')]))
    expect(bom.ok).toBe(false)
    if (!bom.ok) expect(bom.error.startsWith('talaria.json is not valid JSON')).toBe(true)
    expect(parseManifest(enc('{"name":"Plain","surfaces":{"work":"Plain"}}'))).toMatchObject({ ok: true, manifest: { surfaces: { work: 'Plain' } } })
  })
})

describe('docs/sdk/talaria.schema.json', () => {
  it('requires what the validator requires and documents every key it reads', () => {
    expect(schema.required).toEqual(['name'])
    expect(Object.keys(schema.properties).sort()).toEqual([...KNOWN_KEYS].sort())
    expect(schema['x-talaria-sdk-version']).toBe(SDK_VERSION)
  })
  it('holds the slug to the install rule', () => {
    expect(schema.properties.slug?.pattern).toBe(SLUG_RE.source)
    expect(schema.properties.slug?.not?.enum).toEqual([...RESERVED_APP_SLUGS])
    expect([...RESERVED_APP_SLUGS]).toEqual(grants.routes.reservedSlugs)
  })
  it('reads triggers as the validator reads them, case for case', () => {
    expect(schema.properties.triggers?.items?.properties.id?.pattern).toBe(SLUG_RE.source)
    const cases = fixture.manifests.filter((c) => Array.isArray((c.manifest as { triggers?: unknown }).triggers))
    // The fixture's trigger cases: the valid pair and every refusal.
    expect(cases.length).toBeGreaterThanOrEqual(8)
    for (const c of cases) {
      const triggers = (c.manifest as { triggers: unknown[] }).triggers
      const validator = validateManifest({ name: 'X', triggers }).ok
      expect({ about: c.about, schema: triggers.every(schemaAdmitsTrigger) }).toEqual({ about: c.about, schema: validator })
    }
  })
  it('caps the lengths the validator caps, and knows only three surfaces', () => {
    expect(schema.properties.name?.maxLength).toBe(LIMITS.name)
    expect(schema.properties.icon?.maxLength).toBe(LIMITS.icon)
    expect(schema.properties.version?.maxLength).toBe(LIMITS.version)
    expect(schema.properties.description?.maxLength).toBe(LIMITS.description)
    const s = schema.properties.surfaces
    expect(s?.additionalProperties).toBe(false)
    expect(Object.keys(s?.properties ?? {}).sort()).toEqual([...SURFACES].sort())
    for (const k of SURFACES) expect(s?.properties?.[k]?.maxLength).toBe(LIMITS.surfaceTitle)
  })
})
