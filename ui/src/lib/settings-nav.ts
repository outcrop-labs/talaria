// The settings area's navigation: the sidebar's groups, the row that is
// "here", and what a search finds. PURE, like manage-area.ts which it builds
// on: NAV and the page tab tables arrive as arguments, because the modules
// that own them import Svelte files a node-env test cannot load.

import { appAccess, type AppGrantSubject, type ViewerGrants } from '@/lib/app-grants'
import { MANAGE_HOME, manageArea } from '@/lib/manage-area'
import type { NavItem, NavSection } from '@/lib/nav'
import type { AppManifestLike } from '@/lib/nav-sections'
import { activeAmong } from '@/lib/route-tabs'

export const SETTINGS_HOME = '/settings'
export const ADMIN_HOME = '/admin'

export type SettingsNavGroupId = 'personal' | 'workspace' | 'instance'

export interface SettingsNavRow {
  to: string
  label: string
  /** Set for rows that come from NAV or an app manifest. The sidebar keeps
   *  the icons of the rows this module names itself. */
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

export interface SettingsNavMatch {
  to: string
  label: string
  /** The page a tab match belongs to, shown as "Observability › Cost". */
  parent?: string
  icon?: NavItem['icon']
  /** The sidebar row this match belongs to, which is where its icon comes from. */
  rowTo: string
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
  const appSettings = apps.flatMap((a): SettingsNavRow[] =>
    appAccess(a, viewer).settings && a.surfaces.settings
      ? [{ to: appSettingsPath(a.slug), label: a.surfaces.settings, icon: a.icon }]
      : [],
  )
  const personal = [...PERSONAL, ...appSettings]

  const area = manageArea(core, apps, viewer, pathname)
  const workspace: SettingsNavRow[] =
    area.items.length > 0
      ? [{ to: MANAGE_HOME, label: 'Overview' }, ...area.items.map(({ to, label, icon, badge }) => ({ to, label, icon, badge }))]
      : []
  const instance: SettingsNavRow[] = viewer.isAdmin ? [{ to: ADMIN_HOME, label: 'Admin' }] : []

  const groups: SettingsNavGroup[] = [
    { id: 'personal' as const, label: 'Personal', rows: personal },
    { id: 'workspace' as const, label: 'Workspace', rows: workspace },
    { id: 'instance' as const, label: 'Instance', rows: instance },
  ].filter((g) => g.rows.length > 0)

  const outside = activeAmong(pathname, [...personal, ...instance].map((r) => r.to))
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
  '/mcp': ['tools', 'servers', 'connectors', 'integrations'],
  '/templates': ['tickets', 'plans'],
  '/studio': ['skills', 'memory', 'teach'],
  '/observability': ['health', 'cost', 'spend', 'logs', 'usage'],
  '/apps': ['install', 'discover', 'catalog'],
  [ADMIN_HOME]: ['instance', 'organization', 'people', 'users', 'invites', 'secrets', 'security', 'storage', 'git'],
}

/**
 * What a search finds: rows whose label or keywords contain the query, then
 * page tabs whose label does. Only rows the viewer has are searched, so a
 * tab of a page they cannot open never shows.
 */
export function searchSettings(groups: readonly SettingsNavGroup[], tabs: SettingsPageTabs, query: string): SettingsNavMatch[] {
  const q = query.trim().toLowerCase()
  if (!q) return []
  const rows = groups.flatMap((g) => g.rows)
  const has = (text: string) => text.toLowerCase().includes(q)

  const rowMatches = rows
    .filter((r) => has(r.label) || (KEYWORDS[r.to] ?? []).some(has))
    .map((r): SettingsNavMatch => ({ to: r.to, label: r.label, icon: r.icon, rowTo: r.to }))

  const tabMatches = rows.flatMap((r) =>
    (tabs[r.to] ?? []).flatMap((t, i): SettingsNavMatch[] =>
      has(t.label) && t.label.toLowerCase() !== r.label.toLowerCase()
        ? [{ to: i === 0 ? r.to : `${r.to}/${t.id}`, label: t.label, parent: r.label, icon: r.icon, rowTo: r.to }]
        : [],
    ),
  )
  return [...rowMatches, ...tabMatches]
}
