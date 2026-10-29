// Boot sweep + directory watch: compile every installed app whose source
// hash does not match current.json, then load its modules. Failures stay on
// that app, mark it failed, and drop it from the enabled set. An app an
// update holds (update.ts) is left alone until the update lets go.
import { existsSync, watch } from 'node:fs'
import { appSql, recreateIfAliased, stopAppDatabase } from '../app-db'
import { isolateApp } from '../app-isolate'
import { failHealth, probeLoad } from '../app-health'
import { buildApp } from './build'
import { appDir, appsDir, listInstalledApps, slugOk } from './paths'
import { readCurrent } from './status'
import { isHeld } from './update'

const inflight = new Map<string, Promise<void>>()

export async function reconcileApp(slug: string): Promise<void> {
  if (!slugOk(slug) || isHeld(slug)) return
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

/** Host boot: compile + spawn DBs, then watch the apps dir. Never throws.
 *  An app DB from before its compose service was named for its container
 *  answers to `postgres` beside the platform's own, and can take the
 *  platform's connections; those go first, by docker alone, before a build
 *  or a store reads anything from the platform's database. */
export async function startAppBuilds(): Promise<void> {
  for (const app of listInstalledApps()) {
    await isolateApp(app.slug, 'database alias', () => recreateIfAliased(app.slug))
  }
  const swept = await isolateApp('_host', 'app-build sweep', reconcileAll)
  if (!swept.ok) console.error('[app-build] sweep', swept.error)
  try {
    watch(appsDir(), { persistent: true }, (_event, filename) => {
      if (typeof filename !== 'string') return
      const slug = filename.split('/')[0] ?? ''
      if (!slugOk(slug)) return
      // A directory that is there is (re)built: a new app, or one whose
      // manifest changed. One that left apps/ is not built — that only wrote
      // a failed pointer back into app-builds/ after uninstall removed it —
      // and its database stops. Its data stays: the api's uninstall is the
      // one place data is deleted, and only when asked. An update swaps the
      // tree in one step (talaria-apps `exchange`), so an updating app never
      // reads as gone here, and a held one is left to the update.
      if (existsSync(appDir(slug))) {
        void reconcileApp(slug)
        return
      }
      if (!isHeld(slug)) void stopAppDatabase(slug)
    })
  } catch (e) {
    console.error('[app-build] watch', e)
  }
}
