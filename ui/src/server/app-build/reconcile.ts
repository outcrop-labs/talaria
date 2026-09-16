// Boot sweep + directory watch: compile every installed app whose source
// hash does not match current.json, then load its modules. Failures stay on
// that app, mark it failed, and drop it from the enabled set.
import { watch } from 'node:fs'
import { appSql, dropAppDatabase } from '../app-db'
import { isolateApp } from '../app-isolate'
import { failHealth, probeLoad } from '../app-health'
import { buildApp } from './build'
import { appsDir, listInstalledApps, slugOk } from './paths'
import { readCurrent } from './status'

const inflight = new Map<string, Promise<void>>()

export async function reconcileApp(slug: string): Promise<void> {
  if (!slugOk(slug)) return
  const running = inflight.get(slug)
  if (running) return running
  const job = (async () => {
    const built = await isolateApp(slug, 'build', () => buildApp(slug))
    if (!built.ok) {
      await failHealth(slug, built.error)
      return
    }
    const cur = readCurrent(slug)
    if (cur?.status === 'failed') {
      await failHealth(slug, cur.error ?? 'app failed to compile')
      return
    }
    const probed = await probeLoad(slug)
    if (!probed.ok) {
      await failHealth(slug, probed.error)
      return
    }
    const db = await isolateApp(slug, 'database', () => appSql(slug).then(() => undefined))
    if (!db.ok) console.error(`[app-db] ${slug}: ${db.error}`)
  })().finally(() => inflight.delete(slug))
  inflight.set(slug, job)
  return job
}

export async function reconcileAll(): Promise<void> {
  const live = new Set(listInstalledApps().map((a) => a.slug))
  for (const slug of live) await reconcileApp(slug)
}

/** Host boot: compile + spawn DBs, then watch the apps dir. Never throws. */
export async function startAppBuilds(): Promise<void> {
  const swept = await isolateApp('_host', 'app-build sweep', reconcileAll)
  if (!swept.ok) console.error('[app-build] sweep', swept.error)
  try {
    watch(appsDir(), { persistent: true }, (_event, filename) => {
      if (typeof filename !== 'string') return
      const slug = filename.split('/')[0] ?? ''
      if (!slugOk(slug)) return
      void reconcileApp(slug)
      const still = new Set(listInstalledApps().map((a) => a.slug))
      if (!still.has(slug)) void dropAppDatabase(slug)
    })
  } catch (e) {
    console.error('[app-build] watch', e)
  }
}
