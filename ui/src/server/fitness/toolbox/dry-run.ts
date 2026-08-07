// THE DRY RUN — a harness turn with a real tool loop, against a sandbox Talaria.
//
// WHAT IT REPLACES. Three harnesses declare `tools: 'own'` because the tool loop
// IS the feature: an agent working a ticket, a check-in that acts through
// `message_user`, a briefing follow-up the owner expects answered from live
// data. On a fleet persona that loop runs inside the agent container, where the
// platform can see tool NAMES and nothing else. On the org gateway there is no
// loop at all, so the sweep recorded a refusal — a model that was never asked a
// question scoring 0%.
//
// Neither of those measures the thing an admin needs to know, which is not "can
// this model emit a tool call" (the probes answer that in four prompts) but
// "does this model WORK LIKE A COLLEAGUE HERE": read before you write, move the
// status while you work, report an outcome only for something you verified,
// never claim a tool result you did not get. Those are properties of a
// TRANSCRIPT, and this file produces one.
//
// HOW IT DIFFERS FROM PRODUCTION, stated plainly because a benchmark that
// quietly diverges from the thing it predicts is worse than no benchmark:
//
//   the loop is OURS, not the persona's. Production personas run Hermes's loop;
//     this runs a minimal one. What that changes is scaffolding — retries,
//     parallel calls, the agent's own system prompt — and what it preserves is
//     the decision under test: given these tools and this situation, what did
//     the model choose to do.
//   the tools are SANDBOXED. Same names, same descriptions (locked to
//     `mcp/src/index.ts` by a sync test), backends that mutate an in-memory
//     world. A model cannot tell the difference from inside the call.
//   the world is SMALL. Three tickets, one channel, two teammates. Big enough
//     to pose the question, small enough that a fixture's assertions are
//     readable.
//
// IT IS NOT A JUDGE. Nothing here asks a model to grade a model. Every fixture
// that consumes a dry run asserts over `Sandbox.calls` — a list of what happened
// — with plain code.
import type { Message } from '../../harness/define'
import type { ToolCall, Transport, TransportReply, TransportRequest } from '../../harness/transport'

/** WHAT THE LOOP NEEDS OF A SANDBOX, and no more.
 *
 *  There are two — `sandbox.ts` (Talaria's toolkit over an in-memory workspace)
 *  and `hermes-tools.ts` (files and a test runner) — and the loop cares about
 *  neither's world: it offers whatever tools the sandbox declares and hands
 *  every call to `dispatch`. Typed as this narrow pair so a third surface needs
 *  no edit here at all. */
export interface DispatchSandbox {
  tools: Array<{ name: string; description: string; parameters: Record<string, unknown> }>
  dispatch: (call: ToolCall) => Promise<{ text: string; isError: boolean }>
}

/** How many model turns one dry run may take before the loop gives up.
 *
 *  SIX IS A WORKING SESSION, not an agent's whole life. Production work sessions
 *  run to twelve turns; a benchmark case poses one situation, and a model still
 *  circling after six has answered the question being asked of it. The bound is
 *  also what keeps a sweep affordable: an unbounded loop on a chatty model is a
 *  case that costs more than the other twenty-two harnesses combined. */
export const MAX_TURNS = 6

/** Tool calls honored per turn. A model that asks for nine things at once gets
 *  the first three answered and can ask again — which is the same back-pressure
 *  a real gateway applies, and stops one turn from emptying the budget. */
export const MAX_CALLS_PER_TURN = 3

export interface DryRunResult {
  /** The model's LAST reply text — what a caller would have been handed. */
  text: string
  /** Model turns actually taken. */
  turns: number
  /** The loop hit `MAX_TURNS` with the model still calling tools. Not a failure
   *  by itself; a fixture decides whether it is one. */
  exhausted: boolean
  /** The whole conversation, for the drill-down. */
  messages: Message[]
}

/** A TRANSPORT THAT RUNS THE LOOP, so `runHarness` needs no special case.
 *
 *  The runner asks its transport for one reply and gets one — after however many
 *  model turns and tool calls it took to produce it. Everything the runner does
 *  around that (the guard pass, `harness_runs`, the contract, the repair turn)
 *  is unchanged, which is the property that makes a dry run comparable to the
 *  single-shot cases beside it in the same sweep.
 *
 *  `tools: 'own'` IS NEUTRALIZED ON PURPOSE. The harness declares it because
 *  production wants the model's own loop; here the platform IS the loop, so the
 *  request goes down with tool DEFINITIONS instead and `tools: 'none'` — which is
 *  the honest description of what is being sent, and stops `gatewayTransport`
 *  refusing a call this file is about to run itself. */
export function sandboxTransport(sandbox: DispatchSandbox, base: Transport, out?: { result?: DryRunResult }): Transport {
  return async (req: TransportRequest): Promise<TransportReply> => {
    const convo: Message[] = [...req.messages]
    const toolDefs = sandbox.tools.map((t) => ({ name: t.name, description: t.description, parameters: t.parameters }))

    let text = ''
    let turns = 0
    let exhausted = false
    let usage: TransportReply['usage'] = null
    const names: string[] = []

    for (let turn = 0; turn < MAX_TURNS; turn++) {
      turns++
      const reply = await base({ ...req, messages: convo, tools: 'none', toolDefs })
      text = reply.text
      if (reply.usage) {
        usage = usage
          ? { promptTokens: usage.promptTokens + reply.usage.promptTokens, completionTokens: usage.completionTokens + reply.usage.completionTokens }
          : reply.usage
      }

      const calls = (reply.toolCalls ?? []).slice(0, MAX_CALLS_PER_TURN)
      if (calls.length === 0) break
      if (turn === MAX_TURNS - 1) {
        exhausted = true
        break
      }

      // THE MODEL'S OWN TURN GOES INTO THE TRANSCRIPT FIRST, including any prose
      // it wrote alongside the calls. Dropping it would hide the exact failure
      // this suite exists to catch — a model narrating work it then did not do.
      convo.push({ role: 'assistant', content: [reply.text, ...calls.map((c) => `[tool] ${c.name}(${c.args})`)].filter(Boolean).join('\n') })

      for (const call of calls) {
        names.push(call.name)
        const res = await sandbox.dispatch(call)
        convo.push({ role: 'user', content: `Result of ${call.name}:\n${res.text.slice(0, 8_000)}` })
      }
    }

    if (out) out.result = { text, turns, exhausted, messages: convo }
    return { kind: 'gateway', text, toolNames: names, usage, contractDropped: false }
  }
}
