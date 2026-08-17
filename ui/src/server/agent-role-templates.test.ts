// Role templates: the built-ins Talaria maintains, and the org's own on top.
//
// The built-in set is DATA, and data that seeds every agent someone creates is
// worth asserting on — a template whose department is not a legal department, or
// whose soul is missing the sections the muse validator requires, produces an
// agent that fails at creation or a soul the refine step rejects. Both would be
// discovered by a user, not by us.
import { describe, expect, it, vi, beforeEach } from 'vitest'
import { BUILT_IN_ROLE_TEMPLATES } from './agent-role-templates'

let rows: Array<Record<string, unknown>> = []
const sql = (() => Promise.resolve(rows)) as never
vi.mock('@/server/db/pg', () => ({ db: async () => sql }))

const { listRoleTemplates, getRoleTemplate } = await import('./agent-role-templates')

// The shapes fleet-create enforces at agent-creation time.
const SLUG_RE = /^[a-z0-9]+$/
const DEPT_RE = /^[a-z0-9]+(-[a-z0-9]+)*$/
const SOUL_HEADINGS = ['## Who you are', '## Voice & personality', '## How you work']

beforeEach(() => {
  rows = []
})

describe('the built-in library', () => {
  it('ships a usable set of common business roles', () => {
    expect(BUILT_IN_ROLE_TEMPLATES.length).toBeGreaterThanOrEqual(6)
  })

  it('has unique slugs', () => {
    const slugs = BUILT_IN_ROLE_TEMPLATES.map((t) => t.slug)
    expect(new Set(slugs).size).toBe(slugs.length)
  })

  it('names roles, not people', () => {
    // The whole point of the library. A first name here would put an invented
    // person back in front of every new user.
    for (const t of BUILT_IN_ROLE_TEMPLATES) {
      expect(t.name).toBe(t.name.trim())
      expect(t.name.length).toBeGreaterThan(2)
      expect(t.role.length).toBeGreaterThan(2)
    }
  })

  it('declares a department that createAgent would accept', () => {
    for (const t of BUILT_IN_ROLE_TEMPLATES) {
      expect(DEPT_RE.test(t.department), `${t.slug}: ${t.department}`).toBe(true)
    }
  })

  it('yields a handle createAgent would accept once de-hyphenated', () => {
    // The dialog derives the handle from the slug; createAgent's SLUG_RE is
    // alphanumeric only, so a hyphenated template slug must still reduce to a
    // legal handle.
    for (const t of BUILT_IN_ROLE_TEMPLATES) {
      expect(SLUG_RE.test(t.slug.replace(/-/g, '')), t.slug).toBe(true)
    }
  })

  it('writes souls with the sections the muse validator requires', () => {
    for (const t of BUILT_IN_ROLE_TEMPLATES) {
      for (const h of SOUL_HEADINGS) expect(t.soul, `${t.slug} missing ${h}`).toContain(h)
      expect(t.soul.startsWith('# '), t.slug).toBe(true)
    }
  })

  it('carries the human-in-the-loop guarantee in every starting soul', () => {
    // A template is what an operator edits last, so the guarantee has to be in
    // the document they start from rather than only in the platform docs.
    for (const t of BUILT_IN_ROLE_TEMPLATES) {
      expect(t.soul, t.slug).toContain('Keep humans in the loop')
    }
  })

  it('gives every role a one-line description for the picker', () => {
    for (const t of BUILT_IN_ROLE_TEMPLATES) {
      expect(t.description.length, t.slug).toBeGreaterThan(10)
      expect(t.description.length, t.slug).toBeLessThanOrEqual(300)
    }
  })
})

describe('listRoleTemplates', () => {
  it('returns the built-ins when the org has added none', async () => {
    const out = await listRoleTemplates()
    expect(out).toHaveLength(BUILT_IN_ROLE_TEMPLATES.length)
    expect(out.every((t) => t.builtIn)).toBe(true)
  })

  it("includes the org's own, marked as not built-in", async () => {
    rows = [{ slug: 'claims-adjuster', name: 'Claims Adjuster', role: 'Claims Adjuster', department: 'claims', description: 'Ours', soul: '# Claims Adjuster' }]
    const out = await listRoleTemplates()
    expect(out).toHaveLength(BUILT_IN_ROLE_TEMPLATES.length + 1)
    expect(out.find((t) => t.slug === 'claims-adjuster')?.builtIn).toBe(false)
  })

  it('lets an org template SHADOW a built-in of the same slug', async () => {
    const target = BUILT_IN_ROLE_TEMPLATES[0]!
    rows = [{ slug: target.slug, name: 'Our version', role: target.role, department: target.department, description: 'ours', soul: '# Ours' }]
    const out = await listRoleTemplates()
    const matching = out.filter((t) => t.slug === target.slug)
    expect(matching).toHaveLength(1)
    expect(matching[0]!.name).toBe('Our version')
    expect(matching[0]!.builtIn).toBe(false)
  })

  it('resolves a single template by slug, org version winning', async () => {
    const target = BUILT_IN_ROLE_TEMPLATES[0]!
    expect((await getRoleTemplate(target.slug))?.builtIn).toBe(true)
    rows = [{ slug: target.slug, name: 'Our version', role: 'x', department: 'y', description: '', soul: '# Ours' }]
    expect((await getRoleTemplate(target.slug))?.name).toBe('Our version')
  })

  it('answers null for a slug nobody defines', async () => {
    expect(await getRoleTemplate('no-such-role')).toBeNull()
  })
})
