// The one HTTP door for every lib/** query function.
//
// THE RULE: a non-2xx response is an ERROR, never a successful empty value.
// A surface that renders "No boards yet" because /api/boards returned 500 is
// lying to its owner about their data — that lie shipped once and cost a very
// bad afternoon. React Query can only show a real error state if the queryFn
// actually rejects, so this is where rejecting happens.
//
// A 200 carrying `[]` is a genuine empty result and passes straight through:
// empty and broken are different answers and must render differently.
//
// Generalized from the helper `channels.ts` has always had; every query module
// now shares this one.

/** A non-2xx response. `status` lets a call site treat a specific code (a 404
 *  that legitimately means "no such thing") as data rather than failure. */
export class HttpError extends Error {
  readonly status: number
  constructor(status: number, message: string) {
    super(message)
    this.name = 'HttpError'
    this.status = status
  }
}

/** Read a JSON body, throwing on non-2xx with the server's `error` string when
 *  it sent one (API-CONVENTIONS: errors are `{ error: string }`). */
export async function readJson<T>(r: Response): Promise<T> {
  const data = (await r.json().catch(() => null)) as (T & { error?: string }) | null
  // Two different failures, two different sentences. Folding them together
  // produced `HttpError(200, 'request failed (200)')` for a 2xx with an empty
  // or non-JSON body — a status that plainly succeeded, quoted back to the
  // user as the reason it failed. `status` still carries the real code.
  if (!r.ok) throw new HttpError(r.status, data?.error ?? `request failed (${r.status})`)
  if (data === null) throw new HttpError(r.status, 'The server sent a reply this app could not read.')
  return data
}

const SAME_ORIGIN: RequestInit = { credentials: 'same-origin' }

/** GET + parse. Throws on ANY non-2xx, 404 included. */
export async function getJson<T>(url: string, init?: RequestInit): Promise<T> {
  return readJson<T>(await fetch(url, { ...SAME_ORIGIN, ...init }))
}

/** GET + parse where a 404 is a legitimate ANSWER ("no such record, or you
 *  can't see it") rather than a failure: 404 → null, everything else still
 *  throws. Use only where the surface has a real "not found" story to tell. */
export async function getJsonOr404<T>(url: string, init?: RequestInit): Promise<T | null> {
  const r = await fetch(url, { ...SAME_ORIGIN, ...init })
  if (r.status === 404) return null
  return readJson<T>(r)
}

/** GET a list read. API-CONVENTIONS say reads return a single named wrapper
 *  (`{ boards }`, `{ members }`), never a bare array — so a 200 whose wrapper
 *  key is missing or isn't an array is a broken contract, not an empty list,
 *  and throws like any other failure. An actual `[]` passes through. */
export async function getList<T>(url: string, key: string, init?: RequestInit): Promise<T[]> {
  const data = await getJson<Record<string, unknown>>(url, init)
  const list = data[key]
  if (!Array.isArray(list)) throw new Error(`malformed response from ${url}: no "${key}" list`)
  return list as T[]
}

/** One human line for a thrown query error, for error UI. */
export function errorMessage(e: unknown): string {
  const m = e instanceof Error ? e.message.trim() : ''
  if (!m) return 'The server did not respond.'
  // A bare "Failed to fetch" (offline, server restarting) reads as a bug report.
  return /^(failed to fetch|networkerror|load failed)$/i.test(m) ? 'Could not reach the server.' : m
}
