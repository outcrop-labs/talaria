// The app grant rule, pinned: each surface its own grant, the settings tab on
// the manage grant, app views matched exactly while core views keep their
// prefix denial. Every door that serves an app reads these functions.
import { describe, expect, it } from 'vitest'
import { appAccess, appGrantViews, appGrantViewsOf, appViewFor, isViewDenied, reachesApp, type AppGrantSubject } from './app-grants'

const app = (slug: string, surfaces: AppGrantSubject['surfaces']): AppGrantSubject => ({ slug, surfaces })
const full = app('crm', { work: 'CRM', manage: 'CRM Admin', settings: 'CRM' })
const member = (denied: string[]) => ({ isAdmin: false, denied })

describe('appGrantViews', () => {
  it('offers the work view and the manage view as two grants', () => {
    expect(appGrantViews(full)).toEqual([
      { to: '/x/crm', label: 'CRM', surface: 'work' },
      { to: '/x/crm/manage', label: 'CRM Admin', surface: 'manage' },
    ])
  })

  it('gives a settings-only app a manage view to grant, labeled by its settings tab', () => {
    // Before: no view at all, so the tab and its server were open to everyone.
    expect(appGrantViews(app('prefs', { settings: 'Prefs' }))).toEqual([
      { to: '/x/prefs/manage', label: 'Prefs', surface: 'manage' },
    ])
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
  it('lets a manage-only grant through while the work view stays denied', () => {
    const denied = ['/x/crm']
    expect(isViewDenied('/x/crm', denied)).toBe(true)
    expect(isViewDenied('/x/crm/deals', denied)).toBe(true)
    // The bug: the prefix rule took /x/crm/manage down with /x/crm.
    expect(isViewDenied('/x/crm/manage', denied)).toBe(false)
    expect(isViewDenied('/x/crm/manage/fields', denied)).toBe(false)
  })

  it('denies the manage view to a work-only grant, and only that', () => {
    const denied = ['/x/crm/manage']
    expect(isViewDenied('/x/crm/manage', denied)).toBe(true)
    expect(isViewDenied('/x/crm/manage/fields', denied)).toBe(true)
    expect(isViewDenied('/x/crm', denied)).toBe(false)
  })

  it('keeps prefix denial for core views', () => {
    expect(isViewDenied('/boards/abc', ['/boards'])).toBe(true)
    expect(isViewDenied('/boards', ['/boards'])).toBe(true)
    expect(isViewDenied('/boardsx', ['/boards'])).toBe(false)
    expect(isViewDenied('/agents', [])).toBe(false)
  })

  it('never lets one app grant open another app', () => {
    expect(isViewDenied('/x/crm', ['/x/crm', '/x/wiki'])).toBe(true)
    expect(isViewDenied('/x/wiki/manage', ['/x/wiki/manage'])).toBe(true)
    expect(isViewDenied('/x/crm', ['/x/wiki'])).toBe(false)
  })
})

describe('appAccess', () => {
  it('gives an admin every surface the app ships, and only those', () => {
    expect(appAccess(full, { isAdmin: true, denied: [] })).toEqual({ admin: true, work: true, manage: true, settings: true })
    expect(appAccess(app('wiki', { work: 'Wiki' }), { isAdmin: true, denied: [] })).toEqual({
      admin: true,
      work: true,
      manage: false,
      settings: false,
    })
  })

  it('reads each surface from its own grant', () => {
    // Rust denies every app view the member was not granted.
    const none = member(['/x/crm', '/x/crm/manage'])
    const workOnly = member(['/x/crm/manage'])
    const manageOnly = member(['/x/crm'])
    expect(appAccess(full, none)).toEqual({ admin: false, work: false, manage: false, settings: false })
    expect(appAccess(full, workOnly)).toEqual({ admin: false, work: true, manage: false, settings: false })
    // The settings tab follows the manage grant.
    expect(appAccess(full, manageOnly)).toEqual({ admin: false, work: false, manage: true, settings: true })
  })

  it('gates a settings-only app on its manage view', () => {
    const prefs = app('prefs', { settings: 'Prefs' })
    expect(appAccess(prefs, member(['/x/prefs/manage'])).settings).toBe(false)
    expect(appAccess(prefs, member([])).settings).toBe(true)
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
    expect(reachesApp(appAccess(bare, { isAdmin: true, denied: [] }))).toBe(true)
  })
})
