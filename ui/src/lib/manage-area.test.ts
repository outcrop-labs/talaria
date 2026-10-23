// The manage area: the gear and the docked sidebar both read it, so the
// rules are pinned once here. Fixtures are hand-built cores shaped like NAV
// (the real one carries Lucide .svelte icons a node-env test cannot import).
import { describe, expect, it } from 'vitest'
import { manageArea, MANAGE_HOME } from './manage-area'
import type { AppManifestLike } from './nav-sections'
import type { NavSection } from './nav'

const CORE: NavSection[] = [
  { id: 'work', items: [{ to: '/work', label: 'Work', icon: 'work' }] },
  {
    id: 'manage',
    title: 'Manage',
    items: [
      { to: '/agents', label: 'Agents', icon: 'agents' },
      { to: '/models', label: 'Models', icon: 'models' },
      { to: '/observability', label: 'Observability', icon: 'obs' },
    ],
  },
]
const admin = { isAdmin: true, denied: [] as string[], appGrants: [] as string[] }
const crm: AppManifestLike = { slug: 'crm', icon: 'c', surfaces: { work: 'CRM', manage: 'CRM settings' } }

describe('manageArea', () => {
  it('is inside, with the overview active, on /manage', () => {
    const a = manageArea(CORE, [], admin, MANAGE_HOME)
    expect(a.inside).toBe(true)
    expect(a.activePath).toBe('/manage')
    expect(a.items.map((i) => i.to)).toEqual(['/agents', '/models', '/observability'])
  })

  it('is inside on a manage view and on anything nested in one', () => {
    expect(manageArea(CORE, [], admin, '/models')).toMatchObject({ inside: true, activePath: '/models' })
    expect(manageArea(CORE, [], admin, '/observability/costs')).toMatchObject({
      inside: true,
      activePath: '/observability',
    })
  })

  it('is outside on work views — no sidebar there, no lit gear', () => {
    for (const path of ['/work', '/home', '/boards/abc', '/settings', '/admin/people', '/managers']) {
      expect(manageArea(CORE, [], admin, path)).toMatchObject({ inside: false, activePath: null })
    }
  })

  it("counts an app's manage surface as inside, and its work surface as outside", () => {
    expect(manageArea(CORE, [crm], admin, '/x/crm/manage')).toMatchObject({
      inside: true,
      activePath: '/x/crm/manage',
    })
    expect(manageArea(CORE, [crm], admin, '/x/crm')).toMatchObject({ inside: false })
    expect(manageArea(CORE, [crm], admin, '/manage').items.at(-1)).toMatchObject({ to: '/x/crm/manage' })
  })

  it("holds the routes an app adds below each surface to that surface's side", () => {
    expect(manageArea(CORE, [crm], admin, '/x/crm/manage/fields/7')).toMatchObject({
      inside: true,
      activePath: '/x/crm/manage',
    })
    expect(manageArea(CORE, [crm], admin, '/x/crm/items/42')).toMatchObject({ inside: false, activePath: null })
  })

  it("follows the viewer's grants: a denied view is neither listed nor inside", () => {
    const member = { isAdmin: false, denied: ['/models', '/observability'], appGrants: [] }
    const a = manageArea(CORE, [], member, '/models')
    expect(a.items.map((i) => i.to)).toEqual(['/agents'])
    expect(a.inside).toBe(false)
  })

  it("lists an app's manage surface for a member granted only that, and is inside on it", () => {
    // Granted /x/crm/manage alone: the work view stays denied, and it used to
    // take the manage view with it as a prefix — no gear row, no sidebar.
    const member = { isAdmin: false, denied: ['/agents', '/models', '/observability'], appGrants: ['/x/crm/manage'] }
    const a = manageArea(CORE, [crm], member, '/x/crm/manage')
    expect(a.items.map((i) => i.to)).toEqual(['/x/crm/manage'])
    expect(a).toMatchObject({ inside: true, activePath: '/x/crm/manage' })
  })

  it('has nothing at all for a member with no manage grants — not even the overview', () => {
    const member = { isAdmin: false, denied: ['/agents', '/models', '/observability'], appGrants: [] }
    expect(manageArea(CORE, [], member, MANAGE_HOME)).toEqual({ items: [], activePath: null, inside: false })
  })
})
