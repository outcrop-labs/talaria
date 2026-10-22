// The app grant rule, pinned: each surface its own grant, the settings tab on
// the manage grant, app views opened only by a listed grant while core views
// keep their prefix denial. Every door that serves an app reads these
// functions. The cross-runtime half (Rust resolves the grants this reads) is
// server/app-grants.fixture.test.ts.
import { describe, expect, it } from 'vitest'
import { appAccess, appGrantViews, appGrantViewsOf, appViewFor, isViewDenied, reachesApp, type AppGrantSubject } from './app-grants'

const app = (slug: string, surfaces: AppGrantSubject['surfaces'], name?: string): AppGrantSubject => ({ slug, surfaces, ...(name ? { name } : {}) })
const full = app('crm', { work: 'CRM', manage: 'CRM Admin', settings: 'CRM' }, 'CRM')
/** A member holding exactly these app views (the session's `appGrants`). */
const member = (appGrants: string[] | null, denied: string[] = []) => ({ isAdmin: false, denied, appGrants })
const admin = { isAdmin: true, denied: [] as string[], appGrants: [] as string[] }

describe('appGrantViews', () => {
  it('offers the work view and the manage view as two grants', () => {
    expect(appGrantViews(full)).toEqual([
      { to: '/x/crm', label: 'CRM', surface: 'work', title: 'CRM · work' },
      { to: '/x/crm/manage', label: 'CRM Admin', surface: 'manage', title: 'CRM · manage' },
    ])
  })

  it('gives a settings-only app a manage view to grant, labeled by its settings tab', () => {
    // Before: no view at all, so the tab and its server were open to everyone.
    expect(appGrantViews(app('prefs', { settings: 'Prefs' }, 'Prefs'))).toEqual([
      { to: '/x/prefs/manage', label: 'Prefs', surface: 'manage', title: 'Prefs · settings' },
    ])
  })

  it('titles two views of one app apart even when their surfaces share a label', () => {
    // Work "CRM" and settings "CRM" used to read as two identical "CRM" rows.
    const views = appGrantViews(app('crm', { work: 'CRM', settings: 'CRM' }, 'CRM'))
    expect(views.map((v) => v.title)).toEqual(['CRM · work', 'CRM · settings'])
    // No name: the slug stands in.
    expect(appGrantViews(app('wiki', { work: 'Wiki' }))[0]!.title).toBe('wiki · work')
  })

  it('offers nothing for an app with no surface', () => {
    expect(appGrantViews(app('bare', {}))).toEqual([])
  })

  it('lists every work view, then every manage view — the Rust app_view_routes order', () => {
    const views = appGrantViewsOf([full, app('prefs', { settings: 'Prefs' }), app('wiki', { work: 'Wiki' })])
    expect(views.map((v) => v.to)).toEqual(['/x/crm', '/x/wiki', '/x/crm/manage', '/x/prefs/manage'])
  })
})

describe('appViewFor', () => {
  it('names the view that governs an app path', () => {
    expect(appViewFor('/x/crm')).toBe('/x/crm')
    expect(appViewFor('/x/crm/')).toBe('/x/crm')
    expect(appViewFor('/x/crm/deals/7')).toBe('/x/crm')
    expect(appViewFor('/x/crm/manage')).toBe('/x/crm/manage')
    expect(appViewFor('/x/crm/manage/fields')).toBe('/x/crm/manage')
  })

  it('is null off the app routes', () => {
    expect(appViewFor('/boards')).toBeNull()
    expect(appViewFor('/x')).toBeNull()
    expect(appViewFor('/x/')).toBeNull()
    // A work path that merely starts with "manage" is still the work view's.
    expect(appViewFor('/x/crm/managers')).toBe('/x/crm')
  })
})

describe('isViewDenied', () => {
  it('opens a manage-only grant while the work view stays shut', () => {
    const v = member(['/x/crm/manage'])
    expect(isViewDenied('/x/crm', v)).toBe(true)
    expect(isViewDenied('/x/crm/deals', v)).toBe(true)
    // The bug: the prefix rule took /x/crm/manage down with /x/crm.
    expect(isViewDenied('/x/crm/manage', v)).toBe(false)
    expect(isViewDenied('/x/crm/manage/fields', v)).toBe(false)
  })

  it('shuts the manage view to a work-only grant, and only that', () => {
    const v = member(['/x/crm'])
    expect(isViewDenied('/x/crm/manage', v)).toBe(true)
    expect(isViewDenied('/x/crm/manage/fields', v)).toBe(true)
    expect(isViewDenied('/x/crm', v)).toBe(false)
  })

  it('never opens an app view a list left out — absence from the denials is not a grant', () => {
    // The fail-open: Rust could not read the enabled apps, left their views
    // out of deniedViews, and every member got every app. Nothing listed,
    // nothing open, whatever deniedViews says.
    expect(isViewDenied('/x/crm', member([], []))).toBe(true)
    expect(isViewDenied('/x/crm/manage', member([], []))).toBe(true)
  })

  it('holds no app view when the grants could not be read, and keeps the core views', () => {
    // `appGrants: null` — the session could not read the enabled apps. The
    // apps shut; the rest of Talaria answers off deniedViews as ever.
    const unread = member(null, ['/boards'])
    expect(isViewDenied('/x/crm', unread)).toBe(true)
    expect(isViewDenied('/x/crm/manage/fields', unread)).toBe(true)
    expect(isViewDenied('/inbox', unread)).toBe(false)
    expect(isViewDenied('/agents', unread)).toBe(false)
    expect(isViewDenied('/boards/b1', unread)).toBe(true)
  })

  it('opens every app view to an admin by role, with an empty grant list', () => {
    expect(isViewDenied('/x/crm', admin)).toBe(false)
    expect(isViewDenied('/x/crm/manage/fields', admin)).toBe(false)
  })

  it('keeps prefix denial for core views, for admins too', () => {
    expect(isViewDenied('/boards/abc', member([], ['/boards']))).toBe(true)
    expect(isViewDenied('/boards', member([], ['/boards']))).toBe(true)
    expect(isViewDenied('/boardsx', member([], ['/boards']))).toBe(false)
    expect(isViewDenied('/agents', member([]))).toBe(false)
    expect(isViewDenied('/teams', { ...admin, denied: ['/teams'] })).toBe(true)
  })

  it('never lets one app grant open another app', () => {
    expect(isViewDenied('/x/crm', member(['/x/wiki']))).toBe(true)
    expect(isViewDenied('/x/wiki/manage', member(['/x/wiki']))).toBe(true)
    expect(isViewDenied('/x/wiki', member(['/x/wiki']))).toBe(false)
  })
})

describe('appAccess', () => {
  it('gives an admin every surface the app ships, and only those', () => {
    expect(appAccess(full, admin)).toEqual({ admin: true, work: true, manage: true, settings: true })
    expect(appAccess(app('wiki', { work: 'Wiki' }), admin)).toEqual({
      admin: true,
      work: true,
      manage: false,
      settings: false,
    })
  })

  it('reads each surface from its own listed grant', () => {
    expect(appAccess(full, member([]))).toEqual({ admin: false, work: false, manage: false, settings: false })
    expect(appAccess(full, member(['/x/crm']))).toEqual({ admin: false, work: true, manage: false, settings: false })
    // The settings tab follows the manage grant.
    expect(appAccess(full, member(['/x/crm/manage']))).toEqual({ admin: false, work: false, manage: true, settings: true })
  })

  it('grants nothing on a list that could not be read', () => {
    const none = { admin: false, work: false, manage: false, settings: false }
    expect(appAccess(full, member(null))).toEqual(none)
    expect(appAccess(app('prefs', { settings: 'Prefs' }), member(null))).toEqual(none)
    expect(reachesApp(appAccess(full, member(null)))).toBe(false)
  })

  it('gates a settings-only app on its manage view', () => {
    const prefs = app('prefs', { settings: 'Prefs' })
    expect(appAccess(prefs, member([])).settings).toBe(false)
    expect(appAccess(prefs, member(['/x/prefs/manage'])).settings).toBe(true)
  })
})

describe('reachesApp', () => {
  it('admits a caller holding any one surface, and an admin always', () => {
    expect(reachesApp({ admin: false, work: false, manage: false, settings: false })).toBe(false)
    expect(reachesApp({ admin: false, work: false, manage: true, settings: false })).toBe(true)
    expect(reachesApp({ admin: false, work: false, manage: false, settings: true })).toBe(true)
    expect(reachesApp({ admin: true, work: false, manage: false, settings: false })).toBe(true)
  })

  it('admits only admins to an app with nothing to grant', () => {
    const bare = app('bare', {})
    expect(reachesApp(appAccess(bare, member([])))).toBe(false)
    expect(reachesApp(appAccess(bare, admin))).toBe(true)
  })
})
