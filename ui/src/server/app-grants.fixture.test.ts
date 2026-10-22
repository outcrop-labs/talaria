// The app grant rule across the two runtimes — the TS side of the tripwire.
//
// api/tests/it/fixtures/app-grants.json is the ONE artifact both sides assert
// against (Rust: the talaria-users unit tests). Rust resolves each viewer's
// grants to the fixture's `appGrants`; this suite turns that list into
// `access`, so the gate's answer is pinned end to end. It also pins the two
// reads the gate's app list rests on: the stored `apps_enabled` value and
// which manifests are apps at all (a manifest that is not UTF-8 used to be an
// app here and not in Rust).
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { afterEach, describe, expect, it } from 'vitest'
import { appAccess, appGrantViews, appGrantViewsOf, reachesApp, type AppAccess, type AppGrantSubject } from '@/lib/app-grants'
import { enabledSlugsFrom } from './apps'
import { listInstalledApps } from './app-build/paths'

const FIXTURE = join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..', 'api', 'tests', 'it', 'fixtures', 'app-grants.json')

interface Fixture {
  apps: Array<AppGrantSubject & { name: string; views: string[] }>
  enabled: string[]
  appViews: string[]
  viewers: Array<{ about: string; admin?: boolean; appGrants: string[]; access: Record<string, AppAccess> }>
  enabledSetting: Array<{ about: string; absent?: boolean; stored?: unknown; slugs?: string[]; error?: boolean }>
  manifests: Array<{ about: string; dir: string; bytesBase64: string; listed: boolean }>
}

const f = JSON.parse(readFileSync(FIXTURE, 'utf8')) as Fixture

describe('the shared app-grants fixture', () => {
  it('lists each app\'s views as Rust app_grant_views does', () => {
    for (const a of f.apps) expect(appGrantViews(a).map((v) => v.to), a.slug).toEqual(a.views)
  })

  it('lists the enabled apps\' views in Rust\'s app_view_routes order', () => {
    const enabled = f.apps.filter((a) => f.enabled.includes(a.slug))
    expect(appGrantViewsOf(enabled).map((v) => v.to)).toEqual(f.appViews)
  })

  it('turns each viewer\'s listed grants into the access the fixture names', () => {
    expect(f.viewers.length).toBeGreaterThan(0)
    for (const viewer of f.viewers) {
      const isAdmin = viewer.admin === true
      for (const a of f.apps.filter((x) => f.enabled.includes(x.slug))) {
        const want = viewer.access[a.slug] ?? { admin: isAdmin, work: false, manage: false, settings: false }
        const got = appAccess(a, { isAdmin, appGrants: viewer.appGrants })
        expect(got, `${viewer.about}: ${a.slug}`).toEqual(want)
        // Reaching the app is exactly holding something of it (or the role).
        expect(reachesApp(got), `${viewer.about}: ${a.slug}`).toBe(isAdmin || Object.keys(viewer.access).includes(a.slug))
      }
    }
  })

  it('reads the stored apps_enabled value as Rust enabled_slugs_from does', () => {
    for (const c of f.enabledSetting) {
      const stored = c.absent ? undefined : c.stored
      if (c.error) expect(() => enabledSlugsFrom(stored), c.about).toThrow(/not a list of app slugs/)
      else expect(enabledSlugsFrom(stored), c.about).toEqual(c.slugs)
    }
  })

  describe('which manifests are apps', () => {
    const prev = process.env.TALARIA_APPS_DIR
    let dir = ''
    afterEach(() => {
      if (prev === undefined) delete process.env.TALARIA_APPS_DIR
      else process.env.TALARIA_APPS_DIR = prev
      if (dir) rmSync(dir, { recursive: true, force: true })
    })

    it('lists the same apps Rust discovered_apps_in lists', () => {
      dir = mkdtempSync(join(tmpdir(), 'app-grants-fixture-'))
      for (const m of f.manifests) {
        mkdirSync(join(dir, m.dir))
        writeFileSync(join(dir, m.dir, 'talaria.json'), Buffer.from(m.bytesBase64, 'base64'))
      }
      process.env.TALARIA_APPS_DIR = dir
      const listed = listInstalledApps().map((a) => a.slug)
      for (const m of f.manifests) expect(listed.includes(m.dir), m.about).toBe(m.listed)
    })
  })
})
