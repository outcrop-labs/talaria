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
import { runHarness } from './harness/run'
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

const DEADLINE_MS = 10_000

function deadlineSignal(parent?: AbortSignal): { signal: AbortSignal; dispose: () => void } {
  const controller = new AbortController()
  const timeout = setTimeout(() => controller.abort(new DOMException('Inbox assistant timed out', 'TimeoutError')), DEADLINE_MS)
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
 *  from one function is what stops the prompt and the gate from drifting. */
export async function requestFocusCommand(
  model: string,
  input: FocusCommandInput,
  caller: string,
  parentSignal?: AbortSignal,
): Promise<FocusCommandTurn | null> {
  const deadline = deadlineSignal(parentSignal)
  try {
    const result = await runHarness(inboxCommandHarness, input, { caller, model, signal: deadline.signal })
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
