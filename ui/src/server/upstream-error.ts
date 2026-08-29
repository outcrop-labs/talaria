// Fixed sentences for upstream failures that cross a trust boundary (#268).
// An upstream's error body is written by the upstream: it can name its own
// hostnames, its proxy's internals, its account with us — occasionally the
// very credential we sent it. Callers on the far side of a proxy (llm.v1 key
// holders, agent containers) get the STATUS, ours to share, and a fixed
// sentence; the verbatim body goes to the log, which is inside the boundary.
//
// One deliberate exception keeps the wire usable: when the upstream sent a
// STRUCTURED error (OpenAI-style `error.type` / `error.code`), those short
// provider-chosen tokens ride through — clients switch on them for retry
// logic, and they carry no free text. Length-capped, because even a
// structured field is upstream-written.

/** The sentence every upstream failure maps to. The status is the one fact
 *  about the failure that is ours to share. */
export function upstreamErrorMessage(status: number): string {
  return `upstream error (${status})`
}

/** The wire body for a failed upstream hop — fixed sentence, plus structured
 *  type/code when the upstream sent them. Never the free text. */
export function sanitizedUpstreamBody(status: number, body: string): string {
  let type: string | undefined
  let code: string | undefined
  try {
    const j = JSON.parse(body) as { error?: { type?: unknown; code?: unknown } }
    if (typeof j.error?.type === 'string') type = j.error.type.slice(0, 64)
    if (typeof j.error?.code === 'string') code = j.error.code.slice(0, 64)
  } catch {
    /* not JSON — an HTML error page or prose; nothing structured to keep */
  }
  return JSON.stringify({
    error: { message: upstreamErrorMessage(status), ...(type ? { type } : {}), ...(code ? { code } : {}) },
  })
}

/** Log-side helper: the one place the verbatim body is allowed to go. Keeping
 *  the console call here means a future relay path can't "just log it" without
 *  naming this function — and reading this file. */
export function logUpstreamError(where: string, status: number | string, body: string): void {
  console.warn(`[upstream] ${where} ${status}: ${body.slice(0, 500)}`)
}
