// App view grants, in one place. Every reader of an app grant asks here: the
// route gate (AppLayout), the nav and the manage sidebar (nav-sections), the
// settings tabs, the per-person and per-team checklists, the app API gateway
// (/api/apps/<slug>/*) and the /app-builds/ door. PURE — plain arguments in,
// plain data out — so the node-env tests pin it and the server can import it.
//
// THE RULE. Each app surface is its own grant:
//   work      → /x/<slug>
//   manage    → /x/<slug>/manage
//   settings  → follows the manage grant (/x/<slug>/manage), so an app that
//               ships settings but no manage surface still has a view to grant
// Core views deny by prefix ('/boards' denies '/boards/x'). App views do NOT:
// a denied /x/<slug> used to deny /x/<slug>/manage under it, so a grant for
// only an app's Manage surface wrote, audited, and did nothing. The Rust
// authority for which views exist is `app_grant_views` in
// api/crates/talaria-users/src/lib.rs; the denied list itself always comes
// from Rust (`GET /api/auth/session`).

/** The manifest fields a grant decision reads. Structural, so the server's
 *  manifest, the client's and a test fixture all fit. */
export interface AppGrantSubject {
  slug: string
  surfaces: { work?: string; manage?: string; settings?: string }
}

/** Which surfaces of one app a viewer may use. `admin` holds them all. */
export interface AppAccess {
  admin: boolean
  work: boolean
  manage: boolean
  settings: boolean
}

export const appWorkView = (slug: string): string => `/x/${slug}`
export const appManageView = (slug: string): string => `/x/${slug}/manage`

/** One grantable app view, as a checklist offers it. `label` is the surface's
 *  own name from the manifest — the manage label, or the settings label for
 *  an app whose manage grant covers only a settings tab. */
export interface AppGrantView {
  to: string
  label: string
  surface: 'work' | 'manage'
}

/** The views one app contributes to the grant checklists: its work view when
 *  it has a work surface, and its manage view when it has a manage surface OR
 *  a settings tab. */
export function appGrantViews(app: AppGrantSubject): AppGrantView[] {
  const out: AppGrantView[] = []
  if (app.surfaces.work) out.push({ to: appWorkView(app.slug), label: app.surfaces.work, surface: 'work' })
  const manageLabel = app.surfaces.manage ?? app.surfaces.settings
  if (manageLabel) out.push({ to: appManageView(app.slug), label: manageLabel, surface: 'manage' })
  return out
}

/** Every grantable view of these apps: all work views, then all manage views
 *  — the order Rust's `app_view_routes` lists them in. */
export function appGrantViewsOf(apps: readonly AppGrantSubject[]): AppGrantView[] {
  const all = apps.flatMap(appGrantViews)
  return [...all.filter((v) => v.surface === 'work'), ...all.filter((v) => v.surface === 'manage')]
}

const APP_PATH = /^\/x\/([^/]+)(\/manage)?(?:\/|$)/

/** The app view that governs `path`, or null when `path` is not an app route.
 *  `/x/crm/manage` and anything under it is the manage view's; every other
 *  path under `/x/crm` is the work view's. */
export function appViewFor(path: string): string | null {
  const m = APP_PATH.exec(path)
  if (!m?.[1]) return null
  return m[2] ? appManageView(m[1]) : appWorkView(m[1])
}

/** Whether `path` is denied. A core view is denied by itself or by any denied
 *  view above it; an app route is denied only by the exact view that governs
 *  it, so a denied work view never takes the manage view with it. */
export function isViewDenied(path: string, denied: readonly string[]): boolean {
  const app = appViewFor(path)
  if (app !== null) return denied.includes(app)
  return denied.some((d) => path === d || path.startsWith(d + '/'))
}

/** A viewer's access to one app, from the denied list Rust resolved for them
 *  (person + teams). A surface the app doesn't ship is never granted. */
export function appAccess(app: AppGrantSubject, viewer: { isAdmin: boolean; denied: readonly string[] }): AppAccess {
  const { work, manage, settings } = app.surfaces
  const { isAdmin, denied } = viewer
  const workOpen = isAdmin || !denied.includes(appWorkView(app.slug))
  const manageOpen = isAdmin || !denied.includes(appManageView(app.slug))
  return {
    admin: isAdmin,
    work: Boolean(work) && workOpen,
    manage: Boolean(manage) && manageOpen,
    settings: Boolean(settings) && manageOpen,
  }
}

/** Whether a viewer reaches the app at all: an admin always; anyone else
 *  through at least one granted surface. An app with no surface has nothing
 *  to grant, so only admins reach it. */
export const reachesApp = (access: AppAccess): boolean => access.admin || access.work || access.manage || access.settings
