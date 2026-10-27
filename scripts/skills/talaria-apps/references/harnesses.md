# Model calls: harnesses, ctx.harness, ctx.agents

An app never calls a model itself. It declares the call as a **harness** and runs it through
the host's one runner, `ctx.harness`, which goes to the Talaria gateway on the app's own key.
No provider URL, no API key, no transport of your own.

## Declare one — `harnesses/<name>.ts`

One `defineHarness` default export per file under `harnesses/`. While the app is enabled each
file registers: it gets a column in Admin → Models → Fitness, a model setting an admin assigns
there, and the fitness sweep scores candidate models on its `evals`.

```ts
import { defineHarness, z } from '@talaria/sdk/server'

const TRIAGE = z.object({ severity: z.enum(['low', 'medium', 'high']), reason: z.string() })

export default defineHarness<{ subject: string; body: string }, z.infer<typeof TRIAGE>>({
  id: 'support-desk:triage', // unique on the instance: carry the app's name
  label: 'Support triage',
  job: 'Grades an inbound support message so the queue can order itself.',
  requires: ['json', 'instruction-following'],
  floor: { capabilities: [], refuseBelow: false, note: 'Needs a model that returns JSON.' },
  model: { role: 'utility' },
  render: (input) => [
    { role: 'system', content: 'Grade this support message. Reply with JSON: {"severity", "reason"}.' },
    { role: 'user', content: `${input.subject}\n\n${input.body}` },
  ],
  output: { kind: 'json', schema: TRIAGE },
  onFailure: 'null',
  evals: [
    {
      name: 'an outage report grades high',
      input: { subject: 'Checkout is down', body: 'Nobody can pay since 09:00.' },
      check: (v) => (v.severity === 'high' ? null : `graded "${v.severity}" — an outage is high`),
    },
  ],
})
```

| field | what it declares |
| :--- | :--- |
| `id`, `label`, `job` | Who it is. The id must not be a builtin's or another enabled app's (between two apps, the slug that sorts first keeps it) |
| `requires` | Capabilities it leans on. A JSON harness must list `json` |
| `floor` | `{ capabilities, refuseBelow, note }`. Keep `capabilities` empty unless `refuseBelow` is true. A JSON harness gets `json` in its floor and refuses a model probed unable to produce JSON; `note` is the refusal's sentence |
| `model` | Where its model comes from — [Which model](#which-model) |
| `render` | Input → messages. The one thing you write by hand |
| `output` | `{ kind: 'text' }` or `{ kind: 'json', schema, repair? }` (`repair` 0–5) |
| `onFailure` | `'null'`, `'throw'`, `{ fallback }` or `{ escalate: true }` |
| `guard?` | `{ rules?, redact? }`: guardrail rule ids that exist |
| `loop?` | A tool loop the runner holds — [Tool loops](#tool-loops) |
| `evals?` | Fixtures `{ name, input, check, band? }`. `check(value, ctx)` returns null (pass), a string (the model failed) or `{ gap }` (the fixture could not fairly ask). Distinct names |

**A harness that fails a rule is skipped, never the app.** `talaria-app build` names every
harness it registered or skipped, with the reason, as Manage → Apps would.

## Which model

You do not name a model in a definition. Its `model` spec says where one comes from, and the
chain tries each step until one routes:

- **The harness's own setting comes first.** Every app harness has one, `app:<slug>/<harness id>`,
  which an admin assigns beside its column in Admin → Models → Fitness. The host stamps it on
  every run; you never write it.
- **`role`** is the class of work: `utility`, `vision`, `research-brief`, `code-light`, and the
  others Admin → Models lists. The org's model for that role answers next.
- **`pin`** names a **platform agent** (`titler`, `summarizer`, `judge`, …): the model an admin
  assigned that platform job on Models → Platform. It is not a model id and not a fleet agent.
- **`chain`** reorders the steps (`pin`, `slot`, `role`, `utility`, `env`, `preferred`,
  `first-routable`). Leave it out unless you mean it.

A run reports which step answered (`step`). `resolveHarnessModel(spec)` asks the chain without
spending a turn, so a route can answer without the model when nothing routes.

## Run one — `ctx.harness`

`ctx.harness.run(harness, input, options?)` on the request, MCP-tool and trigger contexts.
`ctx.harness.stream(harness, input, { onDelta, ...options })` hands you each piece as it lands
(no repair turn, and a loop does not stream).

```ts
import { defineAppServer, json } from '@talaria/sdk/server'
import triage from './harnesses/triage'

export default defineAppServer({
  async fetch(request, ctx) {
    const { subject, body } = await request.json()
    const run = await ctx.harness.run(triage, { subject, body })
    return json({ severity: run.value?.severity ?? null, model: run.model, error: run.error ?? null })
  },
})
```

The result is `{ value, model, step, answered, refused, repairs, error, … }`. A failure is
`value: null` with the sentence in `error` (or a throw, with `onFailure: 'throw'`).

**The host binds the run to whoever started your code.** It is recorded as
`app:<slug>:<harness id>` and booked to ledger source `app`. There is no `deps`, and nothing
you pass replaces the guard, the transport or the record.

| option | what it does |
| :--- | :--- |
| `model` | Pin this run: a gateway model id, or a fleet agent's id. Checked against the caller (below) |
| `tier` | An agent pin's alias tier (`'opus'`) |
| `effort` | A reasoning effort from the model's published levels |
| `ledger` | `{ taskId?, refId? }`: a ticket and a record of yours the spend also belongs to. UUIDs only |
| `signal` | Your own, on top of the context's |

**Who may pin what.** A refusal makes no model call:

- **From a request:** an agent the signed-in person may use (never someone else's personal
  assistant), or a gateway model their model access allows.
- **From an MCP tool:** only the agent calling the tool, and only when it called on its own key
  (`ctx.caller.proven`).
- **From a trigger:** no agent at all —
  `… a triggered run acts as the app, with no user, and drives no agent through ctx.harness`.
  Any gateway model.

## Tool loops

`loop: { tools, maxTurns? }` makes the runner hold a tool loop: it offers the model your tools,
runs each call and hands the result back, until the model answers (`maxTurns` defaults to 6, at
most 12). Each tool is a `ToolDefinition` (`name`, `description`, `parameters`) plus exactly one
of:

| field | where the call goes |
| :--- | :--- |
| `handler(args, ctx)` | Your code. `ctx` has `app`, `harness`, `actingUserId`, `turn`, `signal` |
| `mcp: '<tool>'` | One of your own `mcp.ts` tools, through the same dispatch an agent's call takes |
| `org: '<server>/<tool>'` | An org MCP tool through the MCP gateway, as the agent the run acts for. Only from an MCP tool whose caller is proven; from a request or a trigger the model is told it is unavailable |

Give every tool a **`simulate(args, { input })`**: the fitness sweep and `talaria-app eval`
dry-run a loop through it, and a tool without one answers the model with an error there. A loop
run's result adds `toolCalls`, `calls` (`{ tool, args, result, error }`) and `exhausted`. A
fixture's `check` can read `ctx.calls` and `ctx.calledBefore(a, b)`. A loop needs a gateway
model that takes tool definitions, not a fleet agent.

## Drive an agent — `ctx.agents.turn`

```ts
const reply = await ctx.agents.turn({
  agent: 'ada-ops', // an id from /api/agents
  message: `Summarise lead ${id} for a handover note.`,
  session: `lead-${id}`, // optional: one conversation the agent keeps
  signal: ctx.signal,
})
// reply: { agent, text, tools, session }
```

Options: `agent`, `message`, `system?`, `session?`, `effort?`, `watch?` (each text and tool event
as it lands), `signal?`. The api decides who may drive whom, from what started your code: from a
request, an agent the signed-in person may use; from an MCP tool, only the calling agent, proven;
from a trigger, none (`app "<slug>" may not drive an agent from its trigger "<id>": …`). It
throws an `HttpError`: 403 with the reason when the caller may not drive that agent or the app is
over its budget, 502 when the agent failed or went silent.

## The app's budget

In production your harness runs, and the fitness sweep's replays of your fixtures, go to the
gateway on the app's own key and count against one budget, `app:<slug>`, which an admin sets in
Admin → Settings → LLM budgets. Over it, a harness run makes no call and comes back with the
gateway's sentence in `error`, and `ctx.agents.turn` starts no turn (a 403):

```
"app:<slug>" has reached its LLM budget: <used> of <cap> in the last <n>h. Requests resume as spend ages out of the window, or an admin can raise the limit (Admin → Settings → LLM budgets).
```

Plan for it: a harness run that comes back `value: null` is an answer your route still has to
give. In your sandbox, `talaria-app eval --gateway` spends on the job's ticket instead, never the
app's budget.

## Test it

`talaria-app eval` replays every fixture of every harness that registers, through the host's
own replay, against the replies in `harnesses/recorded/<file>.json` (no model is called).
`--gateway --model <id>` runs them through the gateway instead; `--record` writes those replies
back. Details: [lifecycle.md](lifecycle.md#eval).

For a unit test, `runHarnessOffline` from `@talaria/sdk/testing` runs the real runner against
replies you script:

```ts
import { expect, test } from 'vitest'
import { runHarnessOffline } from '@talaria/sdk/testing'
import { NO_TOOLS } from '@talaria/sdk/server'
import triage from './harnesses/triage'

test('an outage grades high', async () => {
  const fixture = triage.evals![0]!
  const run = await runHarnessOffline(triage, fixture.input, {
    replies: ['{"severity":"high","reason":"total checkout outage"}'],
  })
  expect(fixture.check(run.value!, NO_TOOLS)).toBeNull()
})
```

A malformed first reply exercises the repair turn; script a tool call as
`{ toolCalls: [{ name, args }] }`.
