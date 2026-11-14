// The settings area's sidebar reads this model, so its rules are pinned here.
// Fixtures are hand-built cores shaped like NAV, as in manage-area.test.ts.
import { describe, expect, it } from 'vitest'
import type { NavSection } from './nav'
import { OWN_PAGE_TABS, searchSettings, settingsNav, type SettingsNavApp, type SettingsPageTabs } from './settings-nav'

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
const TABS: SettingsPageTabs = {
  '/models': [
    { id: 'models', label: 'Models' },
    { id: 'roles', label: 'Roles' },
  ],
  '/observability': [
    { id: 'overview', label: 'Overview' },
    { id: 'cost', label: 'Cost' },
  ],
  '/admin': [
    { id: 'org', label: 'Organization' },
    { id: 'people', label: 'People' },
  ],
}
const admin = { isAdmin: true, denied: [] as string[], appGrants: [] as string[] }
const crm: SettingsNavApp = { slug: 'crm', icon: 'c', surfaces: { work: 'CRM', manage: 'CRM setup', settings: 'CRM' } }

const labels = (nav: ReturnType<typeof settingsNav>) =>
  Object.fromEntries(nav.groups.map((g) => [g.id, g.rows.map((r) => r.label)]))

describe('settingsNav groups', () => {
  it('gives an admin Personal, Workspace and Instance, in that order, and no App settings group with no app', () => {
    const nav = settingsNav(CORE, [], admin, '/manage')
    expect(nav.groups.map((g) => g.label)).toEqual(['Personal', 'Workspace', 'Instance'])
    expect(labels(nav)).toEqual({
      personal: ['Profile', 'Notifications', 'Assistant', 'Connections', 'API keys'],
      workspace: ['Overview', 'Agents', 'Models', 'Observability'],
      instance: ['Admin'],
    })
  })

  it('puts every page an app contributes under App settings, between Workspace and Instance, with the app glyph', () => {
    const nav = settingsNav(CORE, [crm], admin, '/settings')
    expect(nav.groups.map((g) => g.label)).toEqual(['Personal', 'Workspace', 'App settings', 'Instance'])
    expect(nav.groups[2]!.rows).toEqual([
      { to: '/settings/app:crm', label: 'CRM', icon: 'c' },
      { to: '/x/crm/manage', label: 'CRM setup', icon: 'c', badge: undefined },
    ])
    expect(labels(nav).personal).toEqual(['Profile', 'Notifications', 'Assistant', 'Connections', 'API keys'])
    expect(labels(nav).workspace).toEqual(['Overview', 'Agents', 'Models', 'Observability'])
  })

  it("keeps each app's rows together, in the order the apps come", () => {
    const tms: SettingsNavApp = { slug: 'tms', icon: 't', surfaces: { manage: 'Load rules' } }
    const rates: SettingsNavApp = { slug: 'rates', icon: '$', surfaces: { settings: 'Rate Desk' } }
    const nav = settingsNav(CORE, [crm, tms, rates], admin, '/manage')
    expect(labels(nav).apps).toEqual(['CRM', 'CRM setup', 'Load rules', 'Rate Desk'])
  })
})

describe('settingsNav grants', () => {
  it('leaves a member with no manage grants only the Personal group', () => {
    const member = { isAdmin: false, denied: ['/agents', '/models', '/observability'], appGrants: [] }
    const nav = settingsNav(CORE, [crm], member, '/settings')
    expect(nav.groups.map((g) => g.id)).toEqual(['personal'])
  })

  it('drops a denied manage view and never shows Admin to a member', () => {
    const member = { isAdmin: false, denied: ['/models', '/observability'], appGrants: [] }
    const nav = settingsNav(CORE, [], member, '/agents')
    expect(labels(nav).workspace).toEqual(['Overview', 'Agents'])
    expect(nav.groups.some((g) => g.id === 'instance')).toBe(false)
    expect(settingsNav(CORE, [], member, '/admin')).toMatchObject({ inside: false, activePath: null })
  })

  it("shows an app's settings tab on its manage grant, and only then", () => {
    const granted = { isAdmin: false, denied: ['/agents', '/models', '/observability'], appGrants: ['/x/crm/manage'] }
    const theirs = settingsNav(CORE, [crm], granted, '/settings')
    expect(labels(theirs)).toMatchObject({ workspace: ['Overview'], apps: ['CRM', 'CRM setup'] })
    const workOnly = { ...granted, appGrants: ['/x/crm'] }
    expect(settingsNav(CORE, [crm], workOnly, '/settings').groups.map((g) => g.id)).toEqual(['personal'])
  })

  it('holds no app row when the grants could not be read', () => {
    const unreadable = { isAdmin: false, denied: [], appGrants: null }
    const rows = settingsNav(CORE, [crm], unreadable, '/settings').groups.flatMap((g) => g.rows.map((r) => r.to))
    expect(rows).not.toContain('/settings/app:crm')
    expect(rows).not.toContain('/x/crm/manage')
  })
})

describe('settingsNav active row', () => {
  it('lights Profile on /settings and the tab row on a settings tab', () => {
    expect(settingsNav(CORE, [], admin, '/settings')).toMatchObject({ inside: true, activePath: '/settings' })
    expect(settingsNav(CORE, [], admin, '/settings/keys')).toMatchObject({ inside: true, activePath: '/settings/keys' })
    expect(settingsNav(CORE, [crm], admin, '/settings/app:crm').activePath).toBe('/settings/app:crm')
    expect(settingsNav(CORE, [crm], admin, '/x/crm/manage')).toMatchObject({ inside: true, activePath: '/x/crm/manage' })
  })

  it('falls back to Profile on a settings address that names no row, as the page does', () => {
    expect(settingsNav(CORE, [], admin, '/settings/nonsense')).toMatchObject({ inside: true, activePath: '/settings' })
  })

  it('lights Overview on /manage, a manage view on anything nested in it, and Admin on its tabs', () => {
    expect(settingsNav(CORE, [], admin, '/manage').activePath).toBe('/manage')
    expect(settingsNav(CORE, [], admin, '/observability/cost').activePath).toBe('/observability')
    expect(settingsNav(CORE, [], admin, '/admin/people')).toMatchObject({ inside: true, activePath: '/admin' })
  })

  it('is outside on work views', () => {
    for (const path of ['/work', '/home', '/boards/abc', '/x/crm', '/managers']) {
      expect(settingsNav(CORE, [crm], admin, path)).toMatchObject({ inside: false, activePath: null })
    }
  })

  it('is inside on Settings for a member who has no manage view at all', () => {
    const member = { isAdmin: false, denied: ['/agents', '/models', '/observability'], appGrants: [] }
    expect(settingsNav(CORE, [], member, '/settings/notifications')).toMatchObject({
      inside: true,
      activePath: '/settings/notifications',
    })
    expect(settingsNav(CORE, [], member, '/manage').inside).toBe(false)
  })
})

describe('searchSettings', () => {
  const groups = settingsNav(CORE, [crm], admin, '/manage').groups
  const found = (q: string) => searchSettings(groups, TABS, q).map((m) => (m.parent ? `${m.parent} › ${m.label}` : m.label))

  it('finds nothing for an empty query', () => {
    expect(searchSettings(groups, TABS, '   ')).toEqual([])
  })

  it('matches a row by label, whatever the case', () => {
    expect(found('NOTIF')).toEqual(['Notifications'])
  })

  it('matches a row by keyword', () => {
    expect(found('token')).toEqual(['API keys'])
    expect(found('invites')).toEqual(['Admin'])
  })

  it('matches a page tab and links to it', () => {
    expect(searchSettings(groups, TABS, 'roles')).toEqual([
      { to: '/agents', label: 'Agents', icon: 'agents', rowTo: '/agents', group: 'workspace' },
      { to: '/models/roles', label: 'Roles', parent: 'Models', icon: 'models', rowTo: '/models', group: 'workspace' },
    ])
    expect(found('people')).toEqual(['Admin', 'Admin › People'])
  })

  it("links a page's home tab to the page itself", () => {
    expect(searchSettings(groups, TABS, 'organization').at(-1)).toMatchObject({ to: '/admin', parent: 'Admin' })
  })

  it('lists rows before tabs and does not repeat a page as its own tab', () => {
    expect(found('cost')).toEqual(['Observability', 'Observability › Cost'])
    expect(found('models')).toEqual(['Models'])
  })

  it('says which group each match is in, and lists them in the order of the groups', () => {
    const where = (q: string) => searchSettings(groups, TABS, q).map((m) => `${m.group}: ${m.label}`)
    expect(where('crm')).toEqual(['apps: CRM', 'apps: CRM setup'])
    expect(where('model')).toEqual(['personal: Profile', 'workspace: Models'])
    expect(where('setup')).toEqual(['apps: CRM setup'])
  })

  it('never finds a tab of a page the viewer cannot open', () => {
    const member = { isAdmin: false, denied: ['/models', '/observability'], appGrants: [] }
    const theirs = settingsNav(CORE, [], member, '/agents').groups
    expect(searchSettings(theirs, TABS, 'cost')).toEqual([])
    expect(searchSettings(theirs, TABS, 'people')).toEqual([])
  })
})

describe('the tabs of pages with no table of their own', () => {
  const core: NavSection[] = [
    {
      id: 'manage',
      title: 'Manage',
      items: [
        { to: '/agents', label: 'Agents', icon: 'agents' },
        { to: '/mcp', label: 'Tools', icon: 'tools' },
        { to: '/templates', label: 'Templates', icon: 'templates' },
        { to: '/apps', label: 'Apps', icon: 'apps' },
      ],
    },
  ]
  const groups = settingsNav(core, [], admin, '/manage').groups
  const tabLinks = (q: string) =>
    searchSettings(groups, OWN_PAGE_TABS, q)
      .filter((m) => m.parent)
      .map((m) => m.to)

  it('finds every tab of Agents, Tools, Templates and Apps at its own address', () => {
    expect(tabLinks('role templates')).toEqual(['/agents/templates'])
    expect(tabLinks('schedules')).toEqual(['/agents/schedules'])
    expect(tabLinks('built-in')).toEqual(['/mcp'])
    expect(tabLinks('external')).toEqual(['/mcp/external'])
    expect(tabLinks('tickets')).toEqual(['/templates'])
    expect(tabLinks('plans')).toEqual(['/templates/plan'])
    expect(tabLinks('installed')).toEqual(['/apps'])
    expect(tabLinks('discover')).toEqual(['/apps/discover'])
  })

  it('finds Tools by its own name and by the protocol it is served over', () => {
    const rows = (q: string) =>
      searchSettings(groups, OWN_PAGE_TABS, q)
        .filter((m) => !m.parent)
        .map((m) => `${m.label} ${m.to}`)
    for (const q of ['tools', 'mcp', 'MCP', 'model context protocol', 'servers', 'integrations']) {
      expect(rows(q), q).toContain('Tools /mcp')
    }
  })
})
