// The Inbox assistant's four model calls, as adapters over `runHarness`.
//
// WHAT USED TO BE HERE, and why it is gone (audit 1.1, 1.3, 1.4, 1.5)
//   - `parseJsonObject`: indexOf('{') to lastIndexOf('}'), which is a substring
//     rather than a JSON scanner. Verified by execution to fail on three shapes
//     a 14B model emits constantly (a fenced object followed by prose with a
//     brace in it, a preamble then two objects, an object then a bulleted
//     explanation). `harness/json.ts` balances braces and knows what a string
//     literal is.
//   - two request helpers asking for JSON two different ways, chosen by which
//     model the user picked: strict `response_format` at temperature 0.1 on the
//     persona path, a prompt suffix at temperature 0.2 on the gateway path. The
//     runner does one thing on both.
//   - no retry, anywhere, on a malformed structured reply. The runner repairs
//     once with the concrete parser error.
//   - no guardrail on the persona path, while the sibling gateway path was
//     guarded. The same reply was checked or not depending on a dropdown.
//
// WHAT IS STILL HERE, deliberately:
//   the ten-second deadline. `runHarness` has no timeout of its own, and an
//   Inbox turn that hangs is a spinner the owner watches until they give up. It
//   travels as `ctx.signal`, which the fleet transport hands to `proxyChat`.
import { runHarness, runHarnessStreamed } from './harness/run'
import { pickStreamingTransport } from './harness/transport'
import {
  allowedFocusActionIds,
  inboxBriefHarness,
  inboxCommandHarness,
  inboxReplyHarness,
  type FocusBriefInput,
  type FocusCommandInput,
} from './harness/defs/inbox-focus'
import { validBrief, validateCommandObject } from './inbox-focus-policy'
import type { AssistantBrief } from './inbox-focus-types'

/** The cap on a turn NOBODY IS WATCHING: the one-line brief on a queue card,
 *  and the structured command validation behind an action. Both are drawn
 *  alongside other content, both have an honest fallback, and a card that is
 *  ten seconds late is worse than a card that says it could not be written. */
const DEADLINE_MS = 10_000

/** The cap on a turn SOMEBODY IS WAITING FOR — a question they typed into the
 *  assistant panel.
 *
 *  Nine times the brief's, and the asymmetry is the entire point. This deadline
 *  used to be the same 10s, which is shorter than the model takes to answer
 *  anything real: a comparable question on this workspace's own assistant
 *  measures ~23 seconds. So every conversational reply aborted before it
 *  finished and the panel said "your assistant is temporarily unavailable" —
 *  reported, accurately, as the agents not replying. The assistant was never
 *  unavailable; it was interrupted.
 *
 *  A person who has asked a question will wait. What they will not forgive is
 *  being told nobody is home, and — now that the reply actually streams — the
 *  wait is visible as words arriving rather than as a blank panel. */
const REPLY_DEADLINE_MS = 90_000

function deadlineSignal(parent?: AbortSignal, ms = DEADLINE_MS): { signal: AbortSignal; dispose: () => void } {
  const controller = new AbortController()
  const timeout = setTimeout(() => controller.abort(new DOMException('Inbox assistant timed out', 'TimeoutError')), ms)
  const abortFromParent = () => controller.abort(parent?.reason)
  parent?.addEventListener('abort', abortFromParent, { once: true })
  return {
    signal: controller.signal,
    dispose: () => {
      clearTimeout(timeout)
      parent?.removeEventListener('abort', abortFromParent)
    },
  }
}

/** What a validated command turn looks like to `runFocusCommand`. Unchanged
 *  from what `commandFromModel`/`commandFromGatewayModel` returned. */
export interface FocusCommandTurn {
  kind: 'clarification' | 'proposal'
  message: string
  actionId?: string
  payload?: Record<string, unknown>
}

/** One brief for one queue item, on the owner's own assistant.
 *
 *  `validBrief` still runs on the parsed value: the schema proved the shape,
 *  and this clamps the strings to what the card can render and drops a
 *  `recommendedActionId` the item does not offer. Null means the caller keeps
 *  whatever brief it already had — this is fire-and-forget by design. */
export async function requestFocusBrief(
  model: string,
  input: FocusBriefInput,
  caller: string,
  parentSignal?: AbortSignal,
): Promise<AssistantBrief | null> {
  const deadline = deadlineSignal(parentSignal)
  try {
    const result = await runHarness(inboxBriefHarness, input, { caller, model, signal: deadline.signal })
    parentSignal?.throwIfAborted()
    return validBrief(result.value, input.actions)
  } finally {
    deadline.dispose()
  }
}

/** One command turn, on whichever model this seat uses — the owner's persona,
 *  a delegate persona, or a gateway model they picked. The runner chooses the
 *  transport; nothing here has an opinion about it any more.
 *
 *  THE AUTHORITY GATE. `validateCommandObject` runs AFTER the schema parse and
 *  is not negotiable: the schema proved the value has a `message` and an
 *  optional `actionId`, and this proves the `actionId` is one the owner's
 *  instruction actually authorized. The allowlist comes from
 *  `allowedFocusActionIds` — the SAME call `render` made — with the runner's
 *  answer about whether this model earned the widened surface. Deriving both
 *  from one function is what stops the prompt and the gate from drifting.
 *
 *  `effort` is the owner's reasoning-effort pick, already validated against
 *  the answering model's supported levels by the caller; it rides the harness
 *  context to the transport, which sends it where the turn can honor it. */
export async function requestFocusCommand(
  model: string,
  input: FocusCommandInput,
  caller: string,
  parentSignal?: AbortSignal,
  effort?: string | null,
): Promise<FocusCommandTurn | null> {
  const deadline = deadlineSignal(parentSignal)
  try {
    const result = await runHarness(inboxCommandHarness, input, { caller, model, signal: deadline.signal, ...(effort ? { effort } : {}) })
    parentSignal?.throwIfAborted()
    if (!result.value) return null
    return validateCommandObject(result.value, new Set(allowedFocusActionIds(input, result.widened)))
  } finally {
    deadline.dispose()
  }
}

/** A detached conversational reply on a FLEET PERSONA.
 *
 *  Signature preserved exactly: `inbox-focus-conversation.ts` is not this
 *  port's file. `max` is applied to the finished reply rather than to the
 *  stream, which is the one visible difference — truncating a stream mid-token
 *  and truncating an answer at the end produce the same cap and only the second
 *  one can be guarded first. */
export async function requestText(
  model: string,
  messages: Array<{ role: 'system' | 'user' | 'assistant'; content: string }>,
  max = 20_000,
  parentSignal?: AbortSignal,
): Promise<string | null> {
  return replyTurn(model, messages, max, `inbox:${model}`, parentSignal)
}

/** The same reply on an ORG GATEWAY model the owner picked. Two exports rather
 *  than one because both signatures are somebody else's call sites; they are
 *  one harness underneath, and the runner picks the transport from the model. */
export async function requestGatewayText(
  model: string,
  messages: Array<{ role: 'system' | 'user' | 'assistant'; content: string }>,
  caller: string,
  parentSignal?: AbortSignal,
): Promise<string | null> {
  return replyTurn(model, messages, 20_000, caller, parentSignal)
}

async function replyTurn(
  model: string,
  messages: Array<{ role: 'system' | 'user' | 'assistant'; content: string }>,
  max: number,
  caller: string,
  parentSignal?: AbortSignal,
): Promise<string | null> {
  const deadline = deadlineSignal(parentSignal)
  try {
    const result = await runHarness(inboxReplyHarness, { messages }, { caller, model, signal: deadline.signal })
    parentSignal?.throwIfAborted()
    return result.value ? result.value.slice(0, max) || null : null
  } finally {
    deadline.dispose()
  }
}

/** The same conversational reply, STREAMED — deltas as the model writes them.
 *
 *  WHY THIS EXISTS. `requestText` blocks on the whole turn and its caller then
 *  fed the finished string through `chunkText` to fake a stream. From the
 *  panel's side that is not streaming at all: the status line sits there for the
 *  entire turn with nothing under it, and then the whole answer lands at once in
 *  a burst. On a model that takes twenty seconds it reads exactly like the
 *  assistant not replying, which is what it was reported as.
 *
 *  AN ASYNC GENERATOR, because its caller is one. `runHarnessStreamed` pushes
 *  deltas at a callback, and the bridge below turns that into something a
 *  `for await` can pull: deltas queue up, the consumer wakes on each one, and
 *  the RETURN value is the run's own guarded text — not the concatenated
 *  deltas, so a guarded or repaired reply is what gets persisted.
 *
 *  The transport is resolved with `pickStreamingTransport`, which shares its
 *  rule with the blocking `pickTransport`; a fleet persona and an org gateway
 *  model both stream, and neither caller has to know which it got. */
export async function* streamReply(
  model: string,
  messages: Array<{ role: 'system' | 'user' | 'assistant'; content: string }>,
  caller: string,
  opts: { max?: number; parentSignal?: AbortSignal; effort?: string | null } = {},
): AsyncGenerator<string, string | null, void> {
  const deadline = deadlineSignal(opts.parentSignal, REPLY_DEADLINE_MS)
  const stream = await pickStreamingTransport(model)

  const queue: string[] = []
  let wake: (() => void) | null = null
  let settled = false
  let value: string | null = null
  let failure: unknown = null
  const nudge = (): void => {
    const w = wake
    wake = null
    w?.()
  }

  const run = runHarnessStreamed(
    inboxReplyHarness,
    { messages },
    {
      caller,
      model,
      signal: deadline.signal,
      // The owner's effort pick for this reply, validated against this model's
      // supported levels by the caller; the transport forwards it as
      // `reasoning_effort` on whichever side of the house the model lives on.
      ...(opts.effort ? { effort: opts.effort } : {}),
    },
    {
      stream,
      onDelta: (delta) => {
        queue.push(delta)
        nudge()
      },
    },
  )
    .then((result) => {
      value = result.value ? result.value.slice(0, opts.max ?? 20_000) || null : null
    })
    .catch((e: unknown) => {
      failure = e
    })
    .finally(() => {
      settled = true
      nudge()
    })

  try {
    for (;;) {
      while (queue.length > 0) yield queue.shift()!
      if (settled) break
      // One waiter at a time: the generator is pulled by a single consumer, and
      // `nudge` clears the slot before resolving so a delta arriving between the
      // check and the await cannot be missed.
      await new Promise<void>((resolve) => {
        wake = resolve
      })
    }
    await run
    if (failure) throw failure
    opts.parentSignal?.throwIfAborted()
    return value
  } finally {
    deadline.dispose()
  }
}
