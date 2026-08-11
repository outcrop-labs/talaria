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
 *  case that costs more than the other twenty-five harnesses combined.
 *
 *  BRIEFLY RAISED TO TEN AND PUT BACK. The workbench failures that prompted it
 *  turned out to be two bugs in the loop below — a last-turn tool call thrown
 *  away undispatched, and an invented `[tool] name(args)` transcript syntax the
 *  model then imitated as prose. Both are fixed and tested. Raising the budget
 *  was reasoning about a task being too hard, with no measurement behind it, and
 *  it would have masked exactly the defects that were really there. If six turns
 *  is genuinely too few, the evidence for that is a sweep where models exhaust
 *  the loop with the bugs gone — not an argument. */
export const MAX_TURNS = 6

/** THE BUDGET A HARNESS MAY ASK FOR INSTEAD, when six is not what production
 *  gives it. `work-session` runs to `MAX_SESSION_TURNS` (twelve) in production,
 *  so benching it at six measures a shorter job than the one it does — and a
 *  model cut off mid-work is then judged on unfinished work.
 *
 *  Capped, because an unbounded loop on a chatty model is a case that costs more
 *  than the other twenty-five harnesses combined. */
export const MAX_TURN_CEILING = 12
export const turnBudget = (asked?: number): number => Math.max(1, Math.min(asked ?? MAX_TURNS, MAX_TURN_CEILING))

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
export function sandboxTransport(sandbox: DispatchSandbox, base: Transport, out?: { result?: DryRunResult }, maxTurns: number = MAX_TURNS): Transport {
  return async (req: TransportRequest): Promise<TransportReply> => {
    const convo: Message[] = [...req.messages]
    const toolDefs = sandbox.tools.map((t) => ({ name: t.name, description: t.description, parameters: t.parameters }))

    let text = ''
    let turns = 0
    let exhausted = false
    let usage: TransportReply['usage'] = null
    const names: string[] = []

    for (let turn = 0; turn < maxTurns; turn++) {
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

      // THE MODEL'S OWN TURN GOES INTO THE TRANSCRIPT FIRST, including any prose
      // it wrote alongside the calls. Dropping it would hide the exact failure
      // this suite exists to catch — a model narrating work it then did not do.
      //
      // ITS CALLS GO IN THE TOOL CHANNEL, not into its own prose.
      //
      // TWO WORDINGS FAILED BEFORE THIS. First `[tool] write_file({...})`, then
      // `(called write_file)` — both written into the assistant's TEXT because
      // `Message` had nowhere else to put them. Models imitated whichever string
      // they were shown and answered the next turn in prose: 34 replies in one
      // sweep contained Talaria's own narration verbatim, followed by the
      // arguments as text. `reply.toolCalls` came back empty, the loop broke,
      // and the fixture reported "read the repository and never wrote a file"
      // about a model that had written it.
      //
      // Changing the wording only moved the imitation. `Message.toolCalls` and
      // `role: 'tool'` are the shape providers speak and models are trained on,
      // so the conversation now reads back as the tool conversation it was.
      convo.push({ role: 'assistant', content: reply.text, toolCalls: calls })

      // DISPATCHED EVEN ON THE LAST TURN, and this was the other half of the same
      // bug. The budget used to `break` here BEFORE dispatching, so a model that
      // acted on its final turn had that action silently voided — the write never
      // reached the sandbox and `names` never recorded it, so a fixture asking
      // "did it call write_file" was told no about a call plainly in the
      // transcript. A turn budget bounds how many times the model gets to THINK;
      // it must never discard what the model already decided to do.
      for (const call of calls) {
        names.push(call.name)
        const res = await sandbox.dispatch(call)
        convo.push({ role: 'tool', content: res.text.slice(0, 8_000), toolCallId: call.id ?? call.name })
      }

      if (turn === maxTurns - 1) {
        exhausted = true
        break
      }
    }

    // A LAST TURN TO ANSWER IN, and without it the fix above made things worse.
    //
    // Once tool calls actually reached the model, gemma spent its whole budget
    // calling tools — correctly — and the loop ended on a turn whose text was
    // empty, because a turn that calls tools usually says nothing. `clean` is
    // `raw.trim() || null`, so the harness saw no value at all and every
    // workbench case failed the CONTRACT. It had scored 1.00 before only because
    // the model was narrating in prose and doing nothing.
    //
    // So a run that ends still holding tools gets one more call with the tools
    // taken away and its results in front of it: the question stops being "what
    // next" and becomes "so what happened". That is the turn the harness was
    // asking for all along, and never gave.
    if (text.trim().length === 0 && names.length > 0) {
      // WHAT THE CLOSING TURN ASKS DEPENDS ON WHY THE LOOP ENDED, and the first
      // version got this wrong in a way that read as a model failure.
      //
      // It always said "reply with the short summary the instructions asked
      // for". A model that had NOT finished answered, correctly: "I cannot
      // provide the summary because I have not yet fixed the defect. I need to
      // complete the task first." Three workbench cases were then scored as
      // though the model had mis-sequenced its edits — a sentence describing
      // something that never happened, about a model behaving properly under a
      // question it should not have been asked.
      //
      // So an exhausted run is asked what it DID, which it can always answer,
      // and a run that simply stopped calling tools is asked to summarise, which
      // is what the harness prompt promised it would be asked.
      const closingAsk = exhausted
        ? 'You have run out of turns for this session. Do not call any more tools. In one short line, say what you changed and what is still left to do.'
        : 'Stop here and reply with the short summary the instructions asked for. Do not call any more tools.'
      turns++
      const closing = await base({
        ...req,
        messages: [...convo, { role: 'user', content: closingAsk }],
        tools: 'none',
      })
      text = closing.text
      if (closing.usage) {
        usage = usage
          ? { promptTokens: usage.promptTokens + closing.usage.promptTokens, completionTokens: usage.completionTokens + closing.usage.completionTokens }
          : closing.usage
      }
    }

    if (out) out.result = { text, turns, exhausted, messages: convo }
    return { kind: 'gateway', text, toolNames: names, usage, contractDropped: false }
  }
}
