// The section derivation lifted out of NavRail, pinned here so the top dock
// and the manage sidebar inherit the same rules rather than a paraphrase of
// them. The fixtures are hand-built cores shaped like NAV — the real one
// cannot be imported into a node-env test because its items carry Lucide
// icons, which are raw .svelte files (see nav-sections.ts for the full
// story). Icon values are plain strings; the derivation never looks at them.
import { describe, expect, it } from 'vitest'
import { appManageItem, appWorkItem, navActiveItem, navSections, type AppManifestLike } from './nav-sections'
import type { NavSection } from './nav'

const member = { isAdmin: false, denied: [] as string[] }
const admin = { isAdmin: true, denied: [] as string[] }

// Shaped like the real NAV: Work has no title (the views are the label),
// Manage keeps one because it is the control plane.
const CORE: NavSection[] = [
  {
    id: 'work',
    items: [
      { to: '/home', label: 'Inbox', icon: 'inbox' },
      { to: '/comms', label: 'Comms', icon: 'comms' },
      { to: '/boards', label: 'Boards', icon: 'boards' },
    ],
  },
  {
    id: 'manage',
    title: 'Manage',
    items: [
      { to: '/agents', label: 'Agents', icon: 'agents' },
      { to: '/teams', label: 'Teams', icon: 'teams' },
    ],
  },
]

const app = (slug: string, surfaces: { work?: string; manage?: string }): AppManifestLike => ({
  slug,
  icon: 'app-glyph',
  surfaces,
})

describe('navSections', () => {
  it('keeps the core sections as they are declared — Work bare, Manage titled', () => {
    const sections = navSections(CORE, [], [], member)
    expect(sections.map((s) => s.id)).toEqual(['work', 'manage'])
    expect(sections[0]).toMatchObject({ id: 'work' })
    expect(sections[0]!.title).toBeUndefined()
    expect(sections[1]).toMatchObject({ id: 'manage', title: 'Manage' })
  })

  it('slots app manage surfaces under the manage section, by section id', () => {
    const crm = app('crm', { work: 'CRM', manage: 'CRM Admin' })
    const sections = navSections(CORE, [appWorkItem(crm)!], [appManageItem(crm)!], member)
    expect(sections.map((s) => s.id)).toEqual(['work', 'apps', 'manage'])
    const manage = sections.find((s) => s.id === 'manage')
    expect(manage!.items.map((i) => i.to)).toEqual(['/agents', '/teams', '/x/crm/manage'])
    expect(manage!.items[2]).toMatchObject({ label: 'CRM Admin', icon: 'app-glyph' })
  })

  it('forms the Apps section from work surfaces, after Work and ahead of Manage', () => {
    const crm = app('crm', { work: 'CRM', manage: 'CRM Admin' })
    const lead = app('leadworks', { work: 'Leads' })
    const sections = navSections(
      CORE,
      [appWorkItem(crm)!, appWorkItem(lead)!],
      [appManageItem(crm)!],
      member,
    )
    expect(sections.map((s) => s.id)).toEqual(['work', 'apps', 'manage'])
    const apps = sections.find((s) => s.id === 'apps')
    expect(apps).toMatchObject({ title: 'Apps' })
    expect(apps!.items.map((i) => i.to)).toEqual(['/x/crm', '/x/leadworks'])
  })

  it('keeps Apps ahead of Manage when Work is entirely denied — and at the top when nothing core survives', () => {
    const lead = app('leadworks', { work: 'Leads' })
    const workDenied = navSections(CORE, [appWorkItem(lead)!], [], { isAdmin: false, denied: ['/home', '/comms', '/boards'] })
    expect(workDenied.map((s) => s.id)).toEqual(['apps', 'manage'])
    const allDenied = navSections(
      CORE,
      [appWorkItem(lead)!],
      [],
      { isAdmin: false, denied: ['/home', '/comms', '/boards', '/agents', '/teams'] },
    )
    expect(allDenied.map((s) => s.id)).toEqual(['apps'])
  })

  it('drops a section left with nothing and omits the Apps section when no app has a work surface', () => {
    const crm = app('crm', { manage: 'CRM Admin' })
    const sections = navSections(CORE, [], [appManageItem(crm)!], member)
    expect(sections.map((s) => s.id)).toEqual(['work', 'manage'])
    const manageDenied = navSections(CORE, [], [], { isAdmin: false, denied: ['/agents', '/teams'] })
    expect(manageDenied.map((s) => s.id)).toEqual(['work'])
  })

  it('drops adminOnly sections for members and adminOnly items everywhere, keeping them for admins', () => {
    const withAdminOnly: NavSection[] = [
      ...CORE,
      { id: 'system', title: 'System', adminOnly: true, items: [{ to: '/audit', label: 'Audit', icon: 'audit' }] },
    ]
    const mixed: NavSection[] = [
      { id: 'work', items: [{ to: '/home', label: 'Inbox', icon: 'inbox' }, { to: '/internal', label: 'Internal', adminOnly: true, icon: 'internal' }] },
    ]
    expect(navSections(withAdminOnly, [], [], member).map((s) => s.id)).toEqual(['work', 'manage'])
    expect(navSections(withAdminOnly, [], [], admin).map((s) => s.id)).toEqual(['work', 'manage', 'system'])
    expect(navSections(mixed, [], [], member)[0]!.items.map((i) => i.to)).toEqual(['/home'])
    expect(navSections(mixed, [], [], admin)[0]!.items.map((i) => i.to)).toEqual(['/home', '/internal'])
  })

  it('filters by denied views — the view itself and anything nested under it', () => {
    const sections = navSections(CORE, [], [], { isAdmin: false, denied: ['/boards'] })
    expect(sections[0]!.items.map((i) => i.to)).toEqual(['/home', '/comms'])
    // Prefix denial: /boards gone also takes a hypothetical /boards/… item,
    // while a merely-similar prefix (/boardroom) stays.
    const nested: NavSection[] = [
      { id: 'work', items: [{ to: '/boards', label: 'Boards', icon: 'b' }, { to: '/boardroom', label: 'Boardroom', icon: 'r' }] },
    ]
    expect(navSections(nested, [], [], { isAdmin: false, denied: ['/boards'] })[0]!.items.map((i) => i.to)).toEqual(['/boardroom'])
    // Denial does not outrank role: an adminOnly item stays denied even for
    // an admin (deniedViews is empty for admins in practice, but the rule
    // must not depend on that).
    const adminDenied = navSections(CORE, [], [], { isAdmin: true, denied: ['/teams'] })
    expect(adminDenied.find((s) => s.id === 'manage')!.items.map((i) => i.to)).toEqual(['/agents'])
  })

  it('filters app items by the same grants wherever they land', () => {
    const crm = app('crm', { work: 'CRM', manage: 'CRM Admin' })
    const sections = navSections(
      CORE,
      [appWorkItem(crm)!],
      [appManageItem(crm)!],
      { isAdmin: false, denied: ['/x'] },
    )
    expect(sections.map((s) => s.id)).toEqual(['work', 'manage'])
    expect(sections.find((s) => s.id === 'apps')).toBeUndefined()
    // Denying '/x/crm' removes the work item AND its manage surface — the
    // denial is a prefix, so `/x/crm/manage` falls under it, exactly as
    // denying `/boards` hides a task nested in a board. What stays is a
    // DIFFERENT app whose path only shares the `/x` head: `/x/leadworks`
    // does not start with `/x/crm/`.
    const lead = app('leadworks', { work: 'Leads', manage: 'Leads Admin' })
    const workDenied = navSections(
      CORE,
      [appWorkItem(crm)!, appWorkItem(lead)!],
      [appManageItem(crm)!, appManageItem(lead)!],
      { isAdmin: false, denied: ['/x/crm'] },
    )
    expect(workDenied.map((s) => s.id)).toEqual(['work', 'apps', 'manage'])
    expect(workDenied.find((s) => s.id === 'apps')!.items.map((i) => i.to)).toEqual(['/x/leadworks'])
    expect(workDenied.find((s) => s.id === 'manage')!.items.map((i) => i.to)).toEqual(['/agents', '/teams', '/x/leadworks/manage'])
  })
})

describe('navActiveItem', () => {
  it('lights the item whose path contains the route — nested stays lit', () => {
    const sections = navSections(CORE, [], [], member)
    expect(navActiveItem('/boards', sections)).toMatchObject({ to: '/boards' })
    expect(navActiveItem('/boards/some-board/task-9', sections)).toMatchObject({ to: '/boards' })
  })

  it('most specific wins across sections — an app manage surface beats its work surface', () => {
    const crm = app('crm', { work: 'CRM', manage: 'CRM Admin' })
    const sections = navSections(CORE, [appWorkItem(crm)!], [appManageItem(crm)!], member)
    expect(navActiveItem('/x/crm', sections)).toMatchObject({ to: '/x/crm' })
    expect(navActiveItem('/x/crm/manage', sections)).toMatchObject({ to: '/x/crm/manage' })
    // Exactly one item claims the route, not two: the work surface must not
    // stay lit beside its own manage surface.
    expect(sections.flatMap((s) => s.items).filter((i) => i.to === '/x/crm/manage')).toHaveLength(1)
  })

  it('returns null when nothing contains the route', () => {
    const sections = navSections(CORE, [], [], member)
    expect(navActiveItem('/nowhere', sections)).toBeNull()
  })
})

describe('appWorkItem / appManageItem', () => {
  it('maps a manifest surface to a nav entry — path, label, string glyph', () => {
    const crm = app('crm', { work: 'CRM', manage: 'CRM Admin' })
    expect(appWorkItem(crm)).toEqual({ to: '/x/crm', label: 'CRM', icon: 'app-glyph' })
    expect(appManageItem(crm)).toEqual({ to: '/x/crm/manage', label: 'CRM Admin', icon: 'app-glyph' })
  })

  it('returns null for an app without that surface', () => {
    expect(appWorkItem(app('leadworks', { manage: 'Leads Admin' }))).toBeNull()
    expect(appManageItem(app('leadworks', { work: 'Leads' }))).toBeNull()
  })
})