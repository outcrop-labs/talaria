// WHICH APP IS RUNNING — the host's own answer, never the app's.
//
// Every model call the host makes for an app goes to the Rust relay on that
// app's gateway key (app-llm-key.ts, harness/transport.ts). The transport
// therefore has to know whose call it is, and the one thing it must not do is
// ask the app: a caller name an app passes to `runHarness` is a string the app
// chose, and trusting it was the bug (an app could have spent another app's
// budget by naming it). So the host says, at the boundaries where it hands
// control to app code — the app-server dispatch and every other `isolateApp`
// call, an MCP tool handler (app-mcp-call.ts), a fitness replay of one of the
// app's harnesses (app-harnesses.ts) — and the scope follows the async work
// that code starts, however deep, and nowhere else.
//
// Outside every scope there is no app, and a model call is refused: nothing
// in this process makes a model call on its own account any more.
//
// ON globalThis, like the secretbox's keys: a dev HMR reload re-evaluates
// this module, and the app modules a probe loaded before it still reach the
// transport through the host's SDK shim. One store per process, whichever
// copy of this file asks.
import { AsyncLocalStorage } from 'node:async_hooks'
import { slugOk } from './app-build/paths'

interface AppScope {
  slug: string
}

const KEY = Symbol.for('talaria.app-scope')
const store = ((globalThis as Record<symbol, unknown>)[KEY] ??= new AsyncLocalStorage<AppScope>()) as AsyncLocalStorage<AppScope>

/** Run `fn` as app `slug`: every model call made from inside it, at any
 *  depth, is that app's. A slug that is not an app's (the host's own `_host`
 *  sweep label) runs `fn` with no app in scope. */
export function withApp<T>(slug: string, fn: () => T): T {
  if (!slugOk(slug)) return fn()
  return store.run({ slug }, fn)
}

/** The app whose code is running here, or null when none is. */
export function appInScope(): string | null {
  return store.getStore()?.slug ?? null
}
