// The settings area's navigation: the sidebar's groups, the row that is
// "here", and what a search finds. PURE, like manage-area.ts which it builds
// on: NAV and the page tab tables arrive as arguments, because the modules
// that own them import Svelte files a node-env test cannot load.

import { appAccess, type AppGrantSubject, type ViewerGrants } from '@/lib/app-grants'
import { MANAGE_HOME, manageArea } from '@/lib/manage-area'
import type { NavItem, NavSection } from '@/lib/nav'
import { appManageItem, type AppManifestLike } from '@/lib/nav-sections'
import { activeAmong } from '@/lib/route-tabs'

export const SETTINGS_HOME = '/settings'
export const ADMIN_HOME = '/admin'

export type SettingsNavGroupId = 'personal' | 'workspace' | 'apps' | 'instance'

export interface SettingsNavRow {
  to: string
  label: string
  /** Set for rows that come from NAV or an app manifest (the app's own
   *  glyph). The sidebar keeps the icons of the rows this module names. */
  icon?: NavItem['icon']
  badge?: NavItem['badge']
}

export interface SettingsNavGroup {
  id: SettingsNavGroupId
  label: string
  rows: SettingsNavRow[]
}

/** A page's tabs, home tab first. The home tab lives at the page's own path
 *  and every other tab at `<page>/<id>`. */
export type SettingsPageTabs = Readonly<Record<string, ReadonlyArray<{ id: string; label: string }>>>

/** The tabs of the pages that keep no tab table of their own. Each page
 *  renders its strip from here, so what search finds is what the page shows. */
export const AGENTS_TABS = [
  { id: 'agents', label: 'Agents' },
  { id: 'templates', label: 'Role templates' },
  { id: 'schedules', label: 'Schedules' },
] as const
export const MCP_TABS = [
  { id: 'internal', label: 'Built-in' },
  { id: 'external', label: 'External' },
] as const
export const TEMPLATES_TABS = [
  { id: 'ticket', label: 'Tickets' },
  { id: 'plan', label: 'Plans' },
] as const
export const APPS_TABS = [
  { id: 'installed', label: 'Installed' },
  { id: 'discover', label: 'Discover' },
] as const
export const OWN_PAGE_TABS: SettingsPageTabs = {
  '/agents': AGENTS_TABS,
  '/mcp': MCP_TABS,
  '/templates': TEMPLATES_TABS,
  '/apps': APPS_TABS,
}

export interface SettingsNavMatch {
  to: string
  label: string
  /** The page a tab match belongs to, shown as "Observability › Cost". */
  parent?: string
  icon?: NavItem['icon']
  /** The sidebar row this match belongs to, which is where its icon comes from. */
  rowTo: string
  /** The sidebar group that row sits in. Matches come grouped, in the
   *  sidebar's order. */
  group: SettingsNavGroupId
}

export interface SettingsNav {
  groups: SettingsNavGroup[]
  /** The row that is "here", or null outside the area. */
  activePath: string | null
  /** Whether the route is in the settings area. The sidebar shows exactly then. */
  inside: boolean
}

export type SettingsNavApp = AppManifestLike & AppGrantSubject

const PERSONAL: SettingsNavRow[] = [
  { to: SETTINGS_HOME, label: 'Profile' },
  { to: '/settings/notifications', label: 'Notifications' },
  { to: '/settings/assistant', label: 'Assistant' },
  { to: '/settings/connections', label: 'Connections' },
  { to: '/settings/keys', label: 'API keys' },
]

/** The address of an app's settings tab. The Settings page reads it back. */
export const appSettingsPath = (slug: string): string => `/settings/app:${slug}`

export function settingsNav(
  core: NavSection[],
  apps: readonly SettingsNavApp[],
  viewer: ViewerGrants,
  pathname: string,
): SettingsNav {
  // Everything an installed app contributes sits together, app by app: its
  // settings tab, then its manage surface. `manageArea` has already applied
  // the grants, so an app's manage row is here only when it is in its items.
  const area = manageArea(core, apps, viewer, pathname)
  const granted = new Map(area.items.map((item) => [item.to, item]))
  const appManagePaths = new Set<string>()
  const appRows = apps.flatMap((a): SettingsNavRow[] => {
    const rows: SettingsNavRow[] = []
    if (appAccess(a, viewer).settings && a.surfaces.settings) {
      rows.push({ to: appSettingsPath(a.slug), label: a.surfaces.settings, icon: a.icon })
    }
    const manage = appManageItem(a)
    if (manage) {
      appManagePaths.add(manage.to)
      const item = granted.get(manage.to)
      if (item) rows.push({ to: item.to, label: item.label, icon: item.icon, badge: item.badge })
    }
    return rows
  })

  const workspace: SettingsNavRow[] =
    area.items.length > 0
      ? [
          { to: MANAGE_HOME, label: 'Overview' },
          ...area.items.filter((item) => !appManagePaths.has(item.to)).map(({ to, label, icon, badge }) => ({ to, label, icon, badge })),
        ]
      : []
  const instance: SettingsNavRow[] = viewer.isAdmin ? [{ to: ADMIN_HOME, label: 'Admin' }] : []

  const groups: SettingsNavGroup[] = [
    { id: 'personal' as const, label: 'Personal', rows: PERSONAL },
    { id: 'workspace' as const, label: 'Workspace', rows: workspace },
    { id: 'apps' as const, label: 'App settings', rows: appRows },
    { id: 'instance' as const, label: 'Instance', rows: instance },
  ].filter((g) => g.rows.length > 0)

  const appSettings = appRows.filter((r) => !appManagePaths.has(r.to))
  const outside = activeAmong(pathname, [...PERSONAL, ...appSettings, ...instance].map((r) => r.to))
  const activePath = area.activePath ?? outside
  return { groups, activePath, inside: area.inside || outside !== null }
}

const KEYWORDS: Readonly<Record<string, readonly string[]>> = {
  [SETTINGS_HOME]: ['name', 'photo', 'avatar', 'title', 'time zone', 'timezone', 'model', 'account'],
  '/settings/notifications': ['email', 'digest', 'desktop', 'push', 'alerts', 'pause', 'quiet hours'],
  '/settings/assistant': ['agent', 'personality', 'voice', 'schedules', 'skills', 'memory'],
  '/settings/connections': ['google', 'github', 'accounts', 'integrations', 'oauth', 'sign in'],
  '/settings/keys': ['token', 'api', 'scripts', 'cli', 'limits'],
  [MANAGE_HOME]: ['dashboard', 'attention', 'activity', 'restart', 'fleet'],
  '/agents': ['fleet', 'roster', 'roles', 'schedules'],
  '/teams': ['groups', 'members', 'access'],
  '/models': ['providers', 'llm', 'endpoints', 'keys', 'pricing'],
  // The page is called Tools. The protocol's name is how people who know it look for it.
  '/mcp': ['mcp', 'model context protocol', 'servers', 'connectors', 'integrations'],
  '/templates': ['tickets', 'plans'],
  '/studio': ['skills', 'memory', 'teach'],
  '/observability': ['health', 'cost', 'spend', 'logs', 'usage'],
  '/apps': ['install', 'discover', 'catalog'],
  [ADMIN_HOME]: ['instance', 'organization', 'people', 'users', 'invites', 'secrets', 'security', 'storage', 'git'],
}

/**
 * What a search finds: group by group in the sidebar's order, the rows whose
 * label or keywords contain the query, then the page tabs whose label does.
 * Only rows the viewer has are searched, so a tab of a page they cannot open
 * never shows.
 */
export function searchSettings(groups: readonly SettingsNavGroup[], tabs: SettingsPageTabs, query: string): SettingsNavMatch[] {
  const q = query.trim().toLowerCase()
  if (!q) return []
  const has = (text: string) => text.toLowerCase().includes(q)

  return groups.flatMap((g) => {
    const rowMatches = g.rows
      .filter((r) => has(r.label) || (KEYWORDS[r.to] ?? []).some(has))
      .map((r): SettingsNavMatch => ({ to: r.to, label: r.label, icon: r.icon, rowTo: r.to, group: g.id }))

    const tabMatches = g.rows.flatMap((r) =>
      (tabs[r.to] ?? []).flatMap((t, i): SettingsNavMatch[] =>
        has(t.label) && t.label.toLowerCase() !== r.label.toLowerCase()
          ? [{ to: i === 0 ? r.to : `${r.to}/${t.id}`, label: t.label, parent: r.label, icon: r.icon, rowTo: r.to, group: g.id }]
          : [],
      ),
    )
    return [...rowMatches, ...tabMatches]
  })
}
