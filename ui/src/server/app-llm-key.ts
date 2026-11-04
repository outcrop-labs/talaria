// THE KEY AN APP'S MODEL CALLS GO TO THE GATEWAY WITH.
//
// One `tlk_` key per enabled app, named `app:<slug>`, minted by the api on
// this host's ask (`POST /api/internal/apps/<slug>/llm-key`, the internal
// credential), sealed in Postgres there (talaria-apps `llm_key`) and revoked
// when the app is disabled or uninstalled. The relay reads the key as the
// app: its budget, its ledger rows (source `app`, the run's caller stamp),
// no second guard pass. See harness/transport.ts for the calls it signs.
//
// HELD HERE, AND ONLY HERE. The secret lives in this module's map for the
// life of the process and is put on exactly one request header, the relay
// call's. Nothing hands it to app code: no context field, no env var, no SDK
// export. A key the relay refuses (rotated, revoked by a disable-and-enable)
// is forgotten and asked for again, once, by the transport.
//
// On globalThis for the reason app-scope.ts gives: one map per process.
import { INTERNAL_HEADER } from './internal-api'
import { rustApiUrl } from './rust-proxy'
import { internalToken } from './secretbox'

const KEY = Symbol.for('talaria.app-llm-keys')
/** slug → the secret, or the ask in flight for it (two calls that miss at
 *  once share one ask rather than racing the api to mint twice). */
const keys = ((globalThis as Record<symbol, unknown>)[KEY] ??= new Map<string, Promise<string>>()) as Map<string, Promise<string>>

/** How long the api may take to hand a key out: a few queries and, the first
 *  time, one mint — milliseconds on a healthy database, and the call it gates
 *  waits on it, so the bound is generous rather than tight. */
const ASK_MS = 30_000

async function ask(slug: string): Promise<string> {
  const base = rustApiUrl()
  if (base === undefined) {
    throw new Error('the api is stood down in this process (TALARIA_RUST_API_URL=off), so there is no gateway to make a model call through')
  }
  const res = await fetch(`${base}/api/internal/apps/${encodeURIComponent(slug)}/llm-key`, {
    method: 'POST',
    headers: { [INTERNAL_HEADER]: internalToken(), 'content-type': 'application/json' },
    body: '{}',
    redirect: 'manual',
    signal: AbortSignal.timeout(ASK_MS),
  })
  const body = (await res.json().catch(() => null)) as { key?: unknown; error?: unknown } | null
  if (!res.ok || typeof body?.key !== 'string' || !body.key) {
    const why = typeof body?.error === 'string' ? body.error : `the api answered ${res.status}`
    throw new Error(`app "${slug}" has no gateway key: ${why}`)
  }
  return body.key
}

/** The app's gateway key — asked for once and kept. Throws with the api's
 *  sentence when there is none to have (the app is not enabled). */
export function appGatewayKey(slug: string): Promise<string> {
  const held = keys.get(slug)
  if (held) return held
  const asking = ask(slug)
  keys.set(slug, asking)
  // A failed ask is not kept: the next call asks again.
  asking.catch(() => {
    if (keys.get(slug) === asking) keys.delete(slug)
  })
  return asking
}

/** Forget the app's key — the relay refused it, so the api rotated or
 *  revoked it, and the next call asks for the current one. */
export function forgetAppGatewayKey(slug: string): void {
  keys.delete(slug)
}
