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
// only an app's Manage surface wrote, audited, and did nothing.
//
// AN APP VIEW OPENS ONLY ON A LISTED GRANT. `GET /api/auth/session` serves
// `appGrants`, the app views the person holds, and every reader here admits on
// that list — never on a view's absence from `deniedViews`. Reading absence as
// a grant failed open: when Rust could not read the enabled apps it left their
// views out of the denials, and every member got every surface of every app.
// An admin holds every app by role, and their `appGrants` is empty.
//
// WHEN THE APPS CANNOT BE READ. If Rust cannot read the enabled apps (a
// malformed `apps_enabled`, an apps directory it cannot list), the session
// still answers with the core `deniedViews` and `perms`, and serves
// `appGrants: null` with `appsUnreadable: true`. Every reader here holds null
// as no app at all, so a broken app setting costs a member their apps and
// leaves the rest of Talaria open; the app doors answer 503 on it.
//
// The Rust authority for which views exist is `app_grant_views` in
// api/crates/talaria-users/src/lib.rs, and its resolution of a person's
// grants is `resolve_view_grants` there. Both sides are pinned to one shared
// fixture, api/tests/it/fixtures/app-grants.json (server/app-grants.fixture.test.ts
// here, the talaria-users unit tests there).

/** The manifest fields a grant decision reads. Structural, so the server's
 *  manifest, the client's and a test fixture all fit. */
export interface AppGrantSubject {
  slug: string
  /** The app's name, for the checklists' labels; the slug stands in. */
  name?: string
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

/** One grantable app view, as a checklist offers it. */
export interface AppGrantView {
  to: string
  /** The surface's own name from the manifest — the manage label, or the
   *  settings label for an app whose manage grant covers only a settings tab. */
  label: string
  surface: 'work' | 'manage'
  /** What a checklist shows: the app and what the grant opens — `CRM · work`,
   *  `CRM · manage`, or `CRM · settings` when the manage grant covers only a
   *  settings tab. Two views of one app never share it; their surface labels
   *  often do (an app named CRM labels both its work surface and its
   *  settings tab "CRM"). */
  title: string
}

/** The views one app contributes to the grant checklists: its work view when
 *  it has a work surface, and its manage view when it has a manage surface OR
 *  a settings tab. */
export function appGrantViews(app: AppGrantSubject): AppGrantView[] {
  const out: AppGrantView[] = []
  const name = app.name || app.slug
  if (app.surfaces.work) out.push({ to: appWorkView(app.slug), label: app.surfaces.work, surface: 'work', title: `${name} · work` })
  const manageLabel = app.surfaces.manage ?? app.surfaces.settings
  if (manageLabel) {
    const opens = app.surfaces.manage ? 'manage' : 'settings'
    out.push({ to: appManageView(app.slug), label: manageLabel, surface: 'manage', title: `${name} · ${opens}` })
  }
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

/** What Rust resolved for one viewer (`GET /api/auth/session`, person and
 *  teams together). */
export interface ViewerGrants {
  isAdmin: boolean
  /** `deniedViews`: the core views they may not reach. */
  denied: readonly string[]
  /** `appGrants`: the app views they hold. Empty for an admin, who holds
   *  every app by role. `null` when the session could not read the enabled
   *  apps (`appsUnreadable`): they hold no app view at all. */
  appGrants: readonly string[] | null
}

/** Whether `view` is one the viewer holds by grant — never when the grants
 *  could not be read. */
const holds = (viewer: Pick<ViewerGrants, 'appGrants'>, view: string): boolean => viewer.appGrants?.includes(view) ?? false

/** Whether `path` is denied. A core view is denied by itself or by any denied
 *  view above it — for an admin too, whose list is empty. An app route opens
 *  only on the exact view that governs it, held by grant or by an admin's
 *  role, so a work grant never opens the manage view and a view missing from
 *  both lists stays shut. */
export function isViewDenied(path: string, viewer: ViewerGrants): boolean {
  const app = appViewFor(path)
  if (app !== null) return !viewer.isAdmin && !holds(viewer, app)
  return viewer.denied.some((d) => path === d || path.startsWith(d + '/'))
}

/** A viewer's access to one app, from the app views Rust lists as theirs. A
 *  surface the app doesn't ship is never granted, nothing is granted because
 *  a list left it out, and a list that could not be read (`null`) grants
 *  nothing. */
export function appAccess(app: AppGrantSubject, viewer: Pick<ViewerGrants, 'isAdmin' | 'appGrants'>): AppAccess {
  const { work, manage, settings } = app.surfaces
  const { isAdmin } = viewer
  const workOpen = isAdmin || holds(viewer, appWorkView(app.slug))
  const manageOpen = isAdmin || holds(viewer, appManageView(app.slug))
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
