// Which app a mounted surface belongs to.
//
// An app's client used to hard-code its own slug (`appApi('my-app')`), while
// install derives the slug from the repo URL — so an app scaffolded as
// `contacts` and published as `contacts-app` installed as `contacts-app` and
// every call it made 404'd, or reached a different app that happened to own
// the name. The host knows the slug it mounted; AppSurface hands it down here,
// and the SDK's hooks read it, so app code never has to spell it.
import { getContext, setContext } from 'svelte'
import type { AppManifest } from '@/lib/apps'

/** The three places an app can render. */
export type AppSurfaceName = 'work' | 'manage' | 'settings'

/** What a surface knows about the app it belongs to. Fields are read live —
 *  don't destructure at init if the value must follow a refetch. */
export interface AppContext {
  /** The slug THIS instance installed the app under. */
  readonly slug: string
  /** Which surface is rendering. */
  readonly surface: AppSurfaceName
  /** The app's manifest as the host serves it (name, icon, version, surfaces). */
  readonly manifest: AppManifest
}

// A string, not a Symbol: an app chunk built apart from the host still reaches
// the host's SDK module, but a string key cannot be split by a duplicated
// module the way a Symbol would be.
const KEY = 'talaria:app'

/** Host side: AppSurface calls this during its own init. */
export function setAppContext(app: AppContext): void {
  setContext(KEY, app)
}

/** The surrounding app, or undefined outside an app surface. Call during
 *  component init — it reads Svelte context. */
export function getAppContext(): AppContext | undefined {
  return getContext<AppContext | undefined>(KEY)
}
