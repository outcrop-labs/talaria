// The Muse endpoint. ONE route, TWO answers, because the Muse does two
// genuinely different things and used to pretend they were one:
//
//   PROSE (soul, personality, skill, memory, document, template)
//     text/plain, streamed. Tokens landing in the editor as they arrive is the
//     feature, and nothing below changes it.
//
//   JSON (cron, agent, ticket)
//     application/json, VALIDATED HERE. These streamed too, once, and the
//     browser pulled the object back out with a greedy `/\{[\s\S]*\}/` (audit
//     1.1 — the extractor verified to fail on three shapes a 14B model emits
//     constantly). On failure the client got `null` and the button silently did
//     nothing. Through `runHarness` the same call now gets a schema, one repair
//     turn, a guard pass, and a harness_runs row; the client gets a validated
//     value or a sentence saying why not.
//
// THE STREAMING HALF IS A HARNESS TOO — through the runner's own streaming
// entry point, not around it (audit 1.5, the Muse row: these six draft SOULS,
// SKILLS and MEMORIES and ran with no guardrail at all).
//
//   This route used to pump the SSE stream itself with `buildUpstream` /
//   `fetchUpstream` and then hand the accumulated text back to `runHarness`
//   through a replay transport, because the runner was single-shot. It is not
//   any more: `runHarnessStreamed` takes a `StreamingTransport` and `gatewayStream`
//   is the org-gateway one, so the whole pump — the SSE line buffer, the frame
//   parse, the usage scan, the `recordGatewayUsage` row, the dropped-
//   `response_format` signal — has exactly one spelling, in harness/transport.ts,
//   shared with every other streaming surface. What is left here is the part that
//   is genuinely this route's: turning deltas into an HTTP body.
//
//   The replay also had a seam this does not: it resolved the model, then called
//   the gateway, then re-entered the runner with `resolveModel` and `routing`
//   pinned to what it had found. Two lookups, one call, and a `now` override so
//   the latency described the stream rather than the replay. One run does all of
//   it once.
//
//   What the runner does NOT do is redact what was RELAYED, because by then every
//   character is on the wire. Strict-mode redaction happens on the way OUT, chunk
//   by chunk, in `onDelta` — `createStreamRedactor` and the argument on it.
//   `guardrails.ts` says strict mode cleans "what Talaria persists or hasn't yet
//   relayed"; on this path the accumulated stream IS the saved document, so those
//   are one string and the only place to catch it is before it leaves.
import { defineApi } from '@/server/api-route'
import { json } from '@/server/http'
import { z } from 'zod'
import { parseBody, requireUser } from '@/server/api-guard'
import { isJsonKind, type MuseJsonKind, type MuseKind } from '@/server/muse'
import {
  createStreamRedactor,
  museAgentHarness,
  museCronHarness,
  museDraftHarness,
  museTicketHarness,
  type MuseDraftInput,
  type MuseProseInput,
} from '@/server/harness/defs/muse'
import { gatewayStream, runHarness, runHarnessStreamed } from '@/server/harness/run'
import { resolveHarnessModel } from '@/server/harness/model'
import { getGuardConfig, redactSecrets } from '@/server/guardrails'

const Body = z.object({
  kind: z.enum(['soul', 'personality', 'skill', 'memory', 'cron', 'agent', 'document', 'template', 'ticket']),
  instruction: z.string().trim().min(1).max(8_000),
  current: z.string().max(300_000).optional(),
  context: z.string().max(2_000).optional(),
  chat: z
    .array(z.object({ role: z.enum(['user', 'assistant']), content: z.string().max(300_000) }))
    .max(24)
    .optional(),
})

/** What the USER reads when the model could not hold the contract. Written per
 *  kind and pointed at the next thing to try, because "the JSON could not be
 *  parsed" is a fact about the model and not an instruction to a person. The
 *  technical reason travels beside it as `detail`, and the full story — which
 *  model, which chain step, how many repairs — is already on the harness_runs
 *  row by the time this returns. */
const UNUSABLE: Record<MuseJsonKind, string> = {
  cron: 'Muse could not turn that into a scheduled job — try saying when it should run and what it should do each time.',
  agent: 'Muse could not design an agent from that — try adding a sentence about what it should do.',
  ticket: 'Muse could not turn that into a ticket edit — try naming the fields to change.',
}

const NO_MODEL = 'no routable model found — add an endpoint with models on /models first'

// POST → a validated JSON draft (cron / agent / ticket) or a streamed document.
// Runs on the caller's muse model, metered as `platform:muse:<user>`. Any
// signed-in user; what they can DO with the draft is still governed by the save
// endpoints' own authorization.
export const Route = defineApi('/api/muse', {
  POST: async ({ request }) => {
    const user = await requireUser(request)
    if (user instanceof Response) return user
    const body = await parseBody(request, Body)
    if (body instanceof Response) return body
    const kind = body.kind as MuseKind
    const caller = `platform:muse:${user.email ?? user.name ?? user.id}`

    // ── The structured kinds ────────────────────────────────────────────────
    if (isJsonKind(kind)) {
      const input: MuseDraftInput = body
      // `userId` is what arms the member model allowlist inside the chain (see
      // MUSE_MODEL) — a harness run without it would hand a member the
      // expensive model an admin gated. `signal` lets a user who closes the
      // modal stop the call rather than pay for an answer nobody reads.
      const ctx = { caller, userId: user.id, signal: request.signal }
      const res =
        kind === 'cron'
          ? await runHarness(museCronHarness, input, ctx)
          : kind === 'agent'
            ? await runHarness(museAgentHarness, input, ctx)
            : await runHarness(museTicketHarness, input, ctx)

      if (res.value) return json({ value: res.value, model: res.model }, { headers: { 'x-muse-model': res.model ?? '' } })
      // No model at all is a CONFIGURATION problem and the admin needs the real
      // sentence; a model that answered badly is a MODEL problem and the user
      // needs something they can act on. Two failures, two status codes.
      if (!res.model) return json({ error: res.error ?? NO_MODEL }, { status: 400 })
      return json({ error: UNUSABLE[kind], detail: res.error }, { status: 502 })
    }

    // ── The prose kinds: stream ─────────────────────────────────────────────
    const input: MuseProseInput = { ...body, kind }

    // Resolved HERE and handed to the run as a fixed answer, for the two things
    // a header cannot get from a promise: `x-muse-model` has to be on the
    // Response before the first byte, and "nothing routes" has to be a 400 with
    // an admin-readable sentence rather than a stream that opens and closes
    // empty. `step` travels with it so the harness_runs row still records WHICH
    // chain step won — an install limping along on 'first-routable' is a real
    // finding, and pinning `RunContext.model` would erase it.
    const resolved = await resolveHarnessModel({ ...museDraftHarness.model, userId: user.id })
    if (!resolved) return json({ error: NO_MODEL }, { status: 400 })
    const { model } = resolved

    // Read BEFORE the stream opens, because strict mode has to redact on the way
    // out and there is no way back once a chunk is sent. A settings read that
    // fails must not take the draft down with it — the guard is off for this
    // call and the run below is still recorded.
    const strict = (await getGuardConfig().catch(() => null))?.mode === 'strict'
    const redactor = strict ? createStreamRedactor(redactSecrets) : null

    const encoder = new TextEncoder()
    let sink: ReadableStreamDefaultController<Uint8Array> | null = null
    /** The reader went away (closed tab, cancelled draft). Enqueueing after that
     *  throws, and the throw would surface out of `emit` inside the transport as
     *  a failed harness run — a user who changed their mind recorded as a model
     *  that could not hold its contract. */
    let gone = false
    const bytes = new ReadableStream<Uint8Array>({
      start: (c) => {
        sink = c
      },
      cancel: () => {
        gone = true
      },
    })
    /** Deltas out. Enqueued without waiting on `desiredSize`, so this body does
     *  not push back on the upstream the way a `pipeThrough` did — deliberate:
     *  a draft is bounded by the model's own output length, and holding the
     *  transport mid-stream to pace a browser would stall the guard pass and the
     *  metering behind it. */
    const push = (text: string): void => {
      if (text && !gone) sink?.enqueue(encoder.encode(text))
    }

    /** Did anything reach the browser? Decides whether a failed run is still an
     *  HTTP error (nothing sent yet, so say so properly) or a short document
     *  (the status line is long gone). */
    let relayed = false
    let firstDelta = (): void => {}
    const opened = new Promise<void>((resolve) => {
      firstDelta = resolve
    })

    // ONE run, streamed. `gatewayStream` pumps the SSE frames and meters the
    // turn; the runner resolves nothing (pinned above), renders from the
    // definition, guards the completed reply with the harness's own rule set,
    // writes the findings and the harness_runs row, and applies `onFailure`.
    // Nothing about the prompt, the model policy or the guard has a second
    // spelling in this file any more.
    const settled = runHarnessStreamed(museDraftHarness, input, {
      caller,
      // `userId` arms the member model allowlist inside the chain (see
      // MUSE_MODEL) — it is what stops a member being handed the expensive model
      // an admin gated. It travels even though the chain was already answered
      // above, because the run must be correct if the pin below is ever dropped.
      userId: user.id,
      // The chain ran above (the header needs its answer before the first byte),
      // so this hands the ANSWER over rather than asking again. `step` travels
      // with the model so the harness_runs row still records which fallback won
      // — pinning alone would erase it. This used to be spelled
      // `deps: { resolveModel: … }`, which reached into the testing seam from a
      // production path and gave "the model is already known" two spellings.
      model: resolved.model,
      step: resolved.step,
      signal: request.signal,
    }, {
      stream: gatewayStream,
      onDelta: (delta) => {
        relayed = true
        firstDelta()
        // Outside strict mode `push` is the identity and the stream is
        // byte-for-byte what it always was. In strict mode the redactor holds
        // back the tail and cuts only where a secret pattern cannot straddle.
        push(redactor ? redactor.push(delta) : delta)
      },
    })
      .catch(() => null)
      .then((res) => {
        // The held-back tail. Skipping this would truncate every strict-mode
        // draft by its last token.
        if (redactor && !gone) push(redactor.flush())
        if (!gone) sink?.close()
        return res
      })

    // Hold the Response until either the first token or the end of the run. A
    // run that ended without relaying anything never opened a stream, so it can
    // still be answered as an error — which is the difference between "the
    // gateway refused" and a 200 with an empty body that reads to the user as a
    // Muse that did nothing.
    await Promise.race([opened, settled])
    if (!relayed) {
      const res = await settled
      // The model resolved (we pinned it), so anything that failed here is the
      // MODEL or the route, not the configuration — the 502 half of the same
      // split the structured kinds make above.
      return json({ error: res?.error ?? 'the model returned nothing' }, { status: 502 })
    }

    return new Response(bytes, {
      status: 200,
      headers: {
        'content-type': 'text/plain; charset=utf-8',
        'cache-control': 'no-cache',
        'x-muse-model': model,
      },
    })
  },
})
