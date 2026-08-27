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
 *  it sent one (API-CONVENTIONS: errors are `{ error: string }`).
 *
 *  `statusesAsData` lists 4xx codes whose body is a legitimate ANSWER rather
 *  than an error envelope — those resolve to the body instead of throwing
 *  (same idea as `getJsonOr404`, which special-cases 404 → null before it
 *  gets here). Use only where the surface has a real story to tell with the
 *  body, e.g. focus actions answer `{ status: 'stale' }` on 409. */
export async function readJson<T>(r: Response, statusesAsData: number[] = []): Promise<T> {
  const data = (await r.json().catch(() => null)) as (T & { error?: string }) | null
  // Two different failures, two different sentences. Folding them together
  // produced `HttpError(200, 'request failed (200)')` for a 2xx with an empty
  // or non-JSON body — a status that plainly succeeded, quoted back to the
  // user as the reason it failed. `status` still carries the real code.
  if (!r.ok && !statusesAsData.includes(r.status)) throw new HttpError(r.status, data?.error ?? `request failed (${r.status})`)
  if (data === null) throw new HttpError(r.status, 'The server sent a reply this app could not read.')
  return data
}

// The one `credentials: 'same-origin'` fetch stanza in the app. Every lib/**
// query and mutation goes through a function in this file; the invariant
// checker enforces that (search `same-origin-fetch` in check-invariants.mjs).
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

/** GET + parse where specific statuses carry a legitimate ANSWER in the body
 *  (see `readJson`'s `statusesAsData`) — the read twin of `postJsonOr`. E.g.
 *  the Google panels treat 409/502 bodies ("not connected", "Google hiccup")
 *  as data to render, not failures to retry. */
export async function getJsonOr<T>(url: string, statusesAsData: number[], init?: RequestInit): Promise<T> {
  return readJson<T>(await fetch(url, { ...SAME_ORIGIN, ...init }), statusesAsData)
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

// ── Mutations ────────────────────────────────────────────────────────────────
//
// Before these existed, every mutation call site hand-rolled its own
// `fetch(..., { method, credentials, headers, body })` stanza — and the stanzas
// drifted: some read the error body, most didn't, a few resolved the `{error}`
// envelope as if it were the created record. Same rule as the reads: non-2xx
// rejects, with the server's `error` string when it sent one.

async function sendJson<T>(method: string, url: string, body?: unknown, init?: RequestInit, statusesAsData: number[] = []): Promise<T> {
  // FormData goes as-is: the browser sets the multipart boundary, and a
  // content-type header here would strip the boundary out of it.
  const json = body !== undefined && !(body instanceof FormData)
  return readJson<T>(
    await fetch(url, {
      ...SAME_ORIGIN,
      method,
      ...(body !== undefined ? { body: json ? JSON.stringify(body) : (body as BodyInit) } : {}),
      ...(json ? { headers: { 'content-type': 'application/json' } } : {}),
      ...init,
    }),
    statusesAsData,
  )
}

/** POST + parse. */
export function postJson<T>(url: string, body?: unknown, init?: RequestInit): Promise<T> {
  return sendJson<T>('POST', url, body, init)
}

/** PUT + parse. */
export function putJson<T>(url: string, body?: unknown, init?: RequestInit): Promise<T> {
  return sendJson<T>('PUT', url, body, init)
}

/** PATCH + parse. */
export function patchJson<T>(url: string, body?: unknown, init?: RequestInit): Promise<T> {
  return sendJson<T>('PATCH', url, body, init)
}

/** DELETE + parse. Takes an optional body — house delete routes address the
 *  thing in the body (`{ labelId }`), not the path. They answer `{ ok: true }`,
 *  not 204: the parse keeps a failing delete from reading as silent success. */
export function delJson<T>(url: string, body?: unknown, init?: RequestInit): Promise<T> {
  return sendJson<T>('DELETE', url, body, init)
}

/** POST + parse where specific 4xx statuses carry a legitimate ANSWER in the
 *  body (see `readJson`'s `statusesAsData`). */
export function postJsonOr<T>(url: string, body: unknown, statusesAsData: number[], init?: RequestInit): Promise<T> {
  return sendJson<T>('POST', url, body, init, statusesAsData)
}

/** PUT/DELETE twins of `postJsonOr` — e.g. the model-endpoint cascade answers
 *  409 `{needsForce, affected}` on PUT and DELETE, and that body is the answer. */
export function putJsonOr<T>(url: string, body: unknown, statusesAsData: number[], init?: RequestInit): Promise<T> {
  return sendJson<T>('PUT', url, body, init, statusesAsData)
}

export function delJsonOr<T>(url: string, body: unknown, statusesAsData: number[], init?: RequestInit): Promise<T> {
  return sendJson<T>('DELETE', url, body, init, statusesAsData)
}

/** A POST whose reply is a STREAM: the ok Response with a non-null body,
 *  untouched — `readJson` would buffer it. Non-2xx (or an empty body) throws
 *  with the server's `error` string, same as the buffered door. A 200 whose
 *  content-type is JSON still comes back here: callers with a JSON-fallback
 *  story (chat's "queued") read the header themselves. */
export async function postStream(url: string, body?: unknown, init?: RequestInit): Promise<Response & { body: ReadableStream<Uint8Array> }> {
  const json = body !== undefined && !(body instanceof FormData)
  const r = await fetch(url, {
    ...SAME_ORIGIN,
    method: 'POST',
    ...(body !== undefined ? { body: json ? JSON.stringify(body) : (body as BodyInit) } : {}),
    ...(json ? { headers: { 'content-type': 'application/json' } } : {}),
    ...init,
  })
  if (!r.ok || !r.body) {
    const data = (await r.json().catch(() => null)) as { error?: string } | null
    throw new HttpError(r.status, data?.error ?? `request failed (${r.status})`)
  }
  // The null check above is the guarantee; the type just carries it out.
  return r as Response & { body: ReadableStream<Uint8Array> }
}
