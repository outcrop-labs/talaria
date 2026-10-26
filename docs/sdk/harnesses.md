# Activity harnesses — model calls Talaria runs for you

A harness is a **declared** model call: a prompt, an output contract, a model chain, a
failure policy. You declare one with `defineHarness(...)` and run it through the one runner,
`ctx.harness.run` — you never touch a transport, a key, or a retry.

> **Put one harness in each file under `harnesses/`.** Default-export it from
> `apps/<slug>/harnesses/<name>.ts`. While the app is enabled, the harness is checked and
> registered, gets a column in **Admin → Models → Fitness** and a model setting an admin assigns
> there, and the fitness sweep scores candidate models on its `evals`
> ([Registration](#registration--a-column-in-the-fitness-matrix)). In production your
> `server.ts` or `mcp.ts` runs it with `ctx.harness`
> ([Running one](#running-one--ctxharness) below).

```ts
import { defineHarness, z } from '@talaria/sdk/server'

const TRIAGE = z.object({ severity: z.enum(['low', 'medium', 'high']), reason: z.string() })
type Triage = z.infer<typeof TRIAGE>
interface TriageInput {
  subject: string
  body: string
}

export default defineHarness<TriageInput, Triage>({
  id: 'support:triage',
  label: 'Support triage',
  job: 'Grades an inbound support message so the queue can order itself.',
  requires: ['json', 'instruction-following'],
  floor: {
    capabilities: [],
    refuseBelow: false,
    note: 'Runs on any model that can return JSON; a weak one grades more coarsely and the queue stays usable.',
  },
  model: {},
  render: (input) => [
    { role: 'system', content: 'Grade this support message. Reply with severity and a one-line reason.' },
    { role: 'user', content: `${input.subject}\n\n${input.body}` },
  ],
  output: { kind: 'json', schema: TRIAGE },
  onFailure: 'null',
  guard: { rules: ['secret_leak', 'pii_leak'], redact: true },
  evals: [
    {
      name: 'an outage report grades high',
      input: { subject: 'Checkout is down', body: 'Nobody can pay since 09:00. Every card is declined.' },
      check: (v) => (v.severity === 'high' ? null : `graded "${v.severity}" — a total checkout outage is high`),
    },
  ],
})
```

This exact example is compiled and run in `ui/src/sdk/server.test.ts` — a rename anywhere in
the contract breaks the typecheck of the documentation itself.

## The definition

| field | what it declares |
| :--- | :--- |
| `id` / `label` / `job` | Identity; `id` is the `harness` column of the `harness_runs` row each run writes |
| `requires` | Capabilities the job leans on — never blocks; the fitness suite scores against it |
| `floor` | `{ capabilities, refuseBelow, note }` — refuse weak models, or run anyway? A JSON harness gets a derived floor (below) |
| `model` | A `ModelSpec`: `pin` and/or `role` and a `chain` — never a model id. The host adds `slot`, the harness's own model setting ([below](#its-model-setting)) |
| `render` | Input → `Message[]`. **The only thing you write by hand** |
| `output` | `text` (`clean`/`verify`) or `json` (`schema`, `repair`, `verify`) |
| `onFailure` | `'null'` \| `'throw'` \| `{ fallback }` \| `{ escalate: true }` — what a failure means |
| `widen?` | Capability-gated prompt widening; `render`'s `ctx.widened` says which branch |
| `guard?` | `{ rules?, redact? }` — which guardrail rules apply |
| `tools?` | A `ToolPolicy` if the model may use its own tools this turn |
| `toolDefs?` | Tools offered on one turn and executed by nobody: the model's call comes back as a name |
| `loop?` | `{ tools, maxTurns? }`: a tool loop the runner holds, where each call the model makes is dispatched and answered ([below](#tool-loops)) |
| `evals?` | Fixtures. The fitness suite replays them against each candidate model ([below](#evals)) |

`RenderContext` also carries `model` (name the model in small-model prompts) — it is not an
invitation to branch on model ids; that is what capabilities are for.

## The JSON floor

`defineHarness` derives one floor for you, the same one every platform harness gets: when
`output.kind` is `'json'`, `'json'` joins `floor.capabilities` and `refuseBelow` becomes
`true`. The runner then refuses a model that a probe measured, or an admin declared, unable to
produce JSON: the run makes no model call and comes back with `refused: true`, `value: null`
and an `error` that ends with your `floor.note` (`onFailure: 'throw'` throws it). A model nobody
has tested still runs — unknown is not missing — and so does one the gateway only learned to
call without `response_format`. Write the note for that refusal, and keep
`floor.capabilities` empty unless you also set `refuseBelow: true`.

## Evals

Each `EvalCase` is `{ name, input, check, band? }`. `check(value, ctx)` returns `null`
(pass), a string (fail the **model**), or `{ gap }` (fail the **fixture**). Bands are
`'easy' | 'standard' | 'hard'`: easy is the floor for usable-at-all, standard is the job.

The fitness sweep replays every fixture of a registered harness against each candidate model.
The api asks the app host to run the case: the real runner, the candidate pinned, no
`harness_runs` row written. Then it scores the reply the way it scores a platform harness:

- **Contract.** Did the contract hold, on the first try or after the repair turn?
- **Task.** What did your `check` say? A `check` that throws counts as a gap in the fixture, not
  a failure of the model.
- **Floor.** A floor that refuses the candidate counts as a skip.
- **Tool loops.** A harness with a `loop` is dry-run. The host runs it with every tool answered by
  its `simulate`, and the fixture's `check` reads the calls (`ctx.calls`, `ctx.calledBefore`). It
  is scored only on a candidate that can be handed tool definitions, and only when every tool has
  a `simulate`. Otherwise the cases are skipped with the reason, naming the tools that lack one.
  A harness with `tools: 'own'` (a fleet agent's own loop) is scored only on a candidate that runs
  one.

To run fixtures before you ship, use `runHarnessOffline` from `@talaria/sdk/testing`: the real
runner (render, parse, the repair turn, `verify`, the guard) against replies you script, with no
gateway and no database. Then call the fixture's `check` on the value:

```ts
import { runHarnessOffline } from '@talaria/sdk/testing'
import { NO_TOOLS } from '@talaria/sdk/server'
import triage from './harnesses/triage'

const fixture = triage.evals![0]!
const run = await runHarnessOffline(triage, fixture.input, {
  replies: ['{"severity":"high","reason":"total checkout outage"}'],
})
expect(fixture.check(run.value!, NO_TOOLS)).toBeNull()
```

`replies` are the model's turns in order (a malformed first reply exercises the repair turn), or
pass `reply(request)` to answer from the prompt. `capabilities` sets what is on record about the
model, so you can watch your floor refuse. `run.requests` is every request the run made.
`ui/src/sdk/server.test.ts` does exactly this with the example above.

Two exports exist for the trap every text fixture walks into:

- `belowAnswerFloor(text, floor)` — a check that only asserts what the answer must NOT be is
  passed by a model that says almost nothing. Give it a minimum length and words the answer
  had to engage with, and a non-answer scores as one:

  ```ts
  check: (v) =>
    belowAnswerFloor(v.reason, { minChars: 20, mentions: ['checkout'] }) ??
    (v.severity === 'high' ? null : 'an outage is high'),
  ```

- `NO_TOOLS` — the `EvalContext` a single-shot harness receives (`calls: []`, `world: null`).
  Pass it when you invoke a `check` by hand.

`EvalContext` can see what the model **did**: `calls`, `calledBefore(a, b)`, `world`, and
`exhausted` (hit the turn bound still calling tools). Dry-run surfaces (`dryRun.tools`,
`dryRun.world`, `dryRun.workspace`, `dryRun.credentials`) are declared on the definition —
see the deep contract in [HARNESSES.md](../HARNESSES.md).

## Tool loops

`loop` makes the harness a tool loop that the runner holds. The runner offers the model your
tools, dispatches every call it makes and hands each result back. It repeats until the model
answers or the turns run out. Then it holds the final answer to `output`, exactly as for a
single-shot harness:

```ts
import { defineHarness, z } from '@talaria/sdk/server'

const ANSWER = z.object({ customer: z.string(), openOrders: z.number() })

export default defineHarness<{ email: string }, z.infer<typeof ANSWER>>({
  id: 'support:lookup',
  label: 'Customer lookup',
  job: 'Finds the customer and counts their open orders.',
  requires: ['json', 'tools'],
  floor: { capabilities: [], refuseBelow: false, note: 'Needs a model that calls tools.' },
  model: { role: 'utility' },
  render: (i) => [
    { role: 'system', content: 'Find the customer, then count their open orders.' },
    { role: 'user', content: i.email },
  ],
  output: { kind: 'json', schema: ANSWER },
  onFailure: 'null',
  loop: {
    maxTurns: 4,
    tools: [
      {
        name: 'find_customer',
        description: 'Find a customer by email.',
        parameters: { type: 'object', properties: { email: { type: 'string' } }, required: ['email'] },
        handler: async (args, ctx) => {
          const q = encodeURIComponent(String(args.email))
          const res = await fetch(`https://crm.example.test/customers?email=${q}`, { signal: ctx.signal })
          return res.json() // what the model sees, as JSON
        },
        simulate: (args) => ({ id: 'c-1', email: args.email }),
      },
      {
        name: 'open_orders',
        description: "A customer's open orders.",
        parameters: { type: 'object', properties: { customer: { type: 'string' } } },
        mcp: 'orders_open',
        simulate: () => ({ open: 2 }),
      },
    ],
  },
})
```

Each tool is a `ToolDefinition` (`name`, `description`, `parameters`), which is what the model
sees, plus **exactly one** place its call goes:

| field | where the call goes |
| :--- | :--- |
| `handler(args, ctx)` | Your own code, in the app host. `ctx` has `app`, `harness`, `actingUserId`, `turn` and `signal` |
| `mcp: '<tool>'` | One of your app's own `mcp.ts` tools, through the same dispatch an agent's call takes: its input check, its deadline, its result shaping |
| `org: '<server>/<tool>'` | An org MCP tool, through the MCP gateway, under the per-agent rules of the agent the run acts for. Never a direct call |

A tool may add **`simulate(args, { input })`**, a stand-in for dry runs: what it returns is what the
model sees, and nothing real is touched. The fitness sweep scores a loop harness only when every
one of its tools has a `simulate` ([Evals](#evals)).

**How the loop runs.**

- **Calls per turn.** The runner honors at most three calls per turn.
- **The last turn.** A call made on the last turn still runs.
- **Results.** A result reaches the model through the tool channel, cut to 8,000 characters.
- **Failures.** A tool that throws, or a call to a tool you didn't offer, answers the model with
  the sentence. The run carries on.
- **Ending.** If the loop runs out of turns while the model is still calling tools, the model gets
  one closing turn with the tools taken away, asked to answer from what it has. The same happens
  when the model called tools and then said nothing.
- **Defaults.** `maxTurns` defaults to 6, and can be at most 12.
- **Streaming and models.** A loop doesn't stream, and it needs a model that can be handed tool
  definitions: a gateway model, not a fleet agent.

**Where an `org` tool can run.** An org tool runs only as an agent, so the run has to have one.
From an `mcp.ts` tool, the run is the calling agent: the nested call presents that agent's own key
to the MCP gateway, which applies its grants, the owner's allowance, the spend boundary and the
call record. From a request or a trigger, there is no agent, so the model is told the tool is
unavailable and why. The free `runHarness` has no host binding: `mcp` and `org` tools answer that
they need `ctx.harness`. `handler` tools always run.

**What comes back.** A loop run's `HarnessResult` adds three fields:

- `toolCalls`: every call the model made, in order.
- `calls`: each call with what it got back (`{ tool, args, result, error }`). It is the same shape
  a fixture reads, so production and the fitness sweep read one record.
- `exhausted`: the loop hit its turn bound with the model still calling tools.

The guard grounds the answer in what the tools really returned. A link or id the answer cites that
no tool returned is flagged as `ungrounded_ref`.

**Testing it.** `runHarnessOffline` runs a loop against scripted turns. Script a tool call as
`{ toolCalls: [{ name, args }] }`. Handlers run for real, while `mcp` and `org` tools answer
through their `simulate`. Pass `simulate: true` to answer every tool that way, as the fitness
sweep does. A fixture's `check(value, ctx)` can read `ctx.calls`, `ctx.calledBefore(a, b)` and
`ctx.exhausted`.

## Registration — a column in the fitness matrix

Files under `harnesses/` register while the app is enabled.

- **Build.** The app build compiles them into `harnesses.js`, one chunk per file. A file that does
  not compile — a syntax error, an import that does not resolve — is left out and named, and the
  app still builds.
- **Load probe.** On every build and every enable, the probe loads each file and checks its
  default export against the rules the platform holds its own harnesses to:
  - The guard rules it names exist.
  - The floor is a subset of `requires`, and is empty unless it refuses. For a JSON harness that
    means listing `json` in `requires`, because the derived floor refuses on it.
  - Capability names are real.
  - Fixture names are distinct.
  - A widening has a reason.
  - A temperature is between 0 and 1.
  - A JSON harness's `repair` is a whole number from 0 to 5, and the `model` spec's `pin`, `role`
    and `chain` are names and a list of names.
  - The id is not a builtin harness's id, nor one another enabled app registers. Between two
    apps, the slug that sorts first keeps it.
- **A file that does not compile, fails a check, or throws on import,** is skipped and logged once.
  The app stays enabled, and its row in **Manage → Apps** says "N harnesses skipped" with the
  reason for each file. The rows follow the enabled set: enable an app whose slug sorts first and
  claims your id, and your row names the file it took. A disabled app's row says its harnesses are
  not registered.
- **Each harness that passes** gets a column in the fitness matrix, labelled
  `<App name> · <label>`. That column scores this harness alone: it never joins a role, a
  platform agent or a fleet column, whatever its `model` spec names. Your app picks its model.
  The column lives only while the app is enabled.

## Its model setting

Every app harness has a model setting of its own, named `app:<slug>/<harness id>`. An admin sets it
under the fitness matrix in **Admin → Models → Fitness**, beside the column that scores models on
your fixtures. When it is set, your harness runs on that model before anything its `model` spec
declares: the `slot` step resolves ahead of `role` in any chain, `[]` included. When it is unset, or
names a model that no longer routes, your spec carries on as written. The run records `step: 'slot'`
when the setting answered.

You don't declare it. The host sets `model.slot` from your slug and the harness id on every run,
and overwrites anything your spec put there, so a harness can never borrow another app's setting.

## Running one — `ctx.harness`

This is how an app harness runs in production. Your `server.ts` route, or an `mcp.ts` tool,
imports it and runs it with `ctx.harness`: run the harness when a model chain is routable, fall
back to your own deterministic answer when it isn't, and record which one answered.
`@/server/harness/run` does not resolve from app code **by design**; the context is the road in:

```ts
import { defineAppServer, json, resolveHarnessModel } from '@talaria/sdk/server'
import triage from './harnesses/triage'

export default defineAppServer({
  async fetch(request, ctx) {
    if (ctx.path !== 'triage' || request.method !== 'POST')
      return json({ error: 'not found' }, { status: 404 })
    const { ticketId, subject, body } = await request.json()

    // Free probe — no model call: nothing routable means answer it yourself.
    if (!(await resolveHarnessModel(triage.model)))
      return json({ severity: null, reason: '', source: 'fallback' })

    const run = await ctx.harness.run(triage, { subject, body }, {
      ledger: { taskId: ticketId },
    })
    // run: HarnessResult<Triage> — value, model, step, answered, refused, repairs…
    return json({
      severity: run.value?.severity ?? null,
      reason: run.value?.reason ?? '',
      source: run.value ? `model:${run.model}` : 'fallback',
    })
  },
})
```

**The host binds the runner to the caller.** You don't name yourself or choose where the spend goes:

- **Who it is.** Every run is recorded as `app:<slug>:<harness id>`, and its spend is booked to
  ledger source `app`.
- **Which model.** The run gets the harness's own model setting first.
- **Who it acts for.** The run acts for the person the context belongs to: the signed-in member on
  a request, or the person an agent acts for on a tool call.
- **The guard and the recorders.** There is no `deps`. Nothing you pass replaces the guard, the
  transport or the run record.
- **The rules.** The definition is held to the same rules registration applies, JSON floor
  included, whether or not you built it with `defineHarness`.

| option | what it does |
| :--- | :--- |
| `model` | Pin the model: a gateway model id, or a fleet agent's id (or one of its tier ids). Checked, below |
| `tier` | Route an agent pin to one of its alias tiers (`'opus'`), priced as that tier |
| `effort` | A reasoning effort the person picked, from the model's published levels |
| `ledger` | `{ refId?, taskId? }` — a record of yours and a ticket the spend also belongs to. Both are UUIDs; anything else is left off the row, with a warning |
| `signal` | Your own signal, on top of the one the host passes. A run already stops when the caller disconnects, or at a tool's deadline |

**A pin is checked against the caller.** Each refusal makes no model call. It returns
`value: null` with the reason in `error`, or throws it when your harness declares
`onFailure: 'throw'`.

- **From a request.** You may pin an agent the signed-in member may use: the same answer
  `GET /api/agents` gives them. Pinning someone else's personal assistant is refused. You may pin a
  gateway model only if the member model allowlist opens it to them. An admin may pin any model.
- **From an MCP tool.** You may pin only the agent calling the tool, and only when it called on its
  own key (`caller.proven`). A gateway model pin is held to the allowlist of the person the agent
  acts for. An org agent acts for no one, so its pins are not gated.
- **From a trigger** (a scheduled or event run, which acts as the app with no user). You may not
  pin an agent. Any gateway model is allowed.

**Streaming.** `ctx.harness.stream(def, input, { onDelta, ...options })` hands you each piece as it
lands, then resolves with the same `HarnessResult`. A streamed run makes no repair attempt: the
first answer is already on the screen. `onDelta` gets the raw reply before the guard pass. If you
must scrub what you relay, scrub it there.

| export | what it is |
| :--- | :--- |
| `ctx.harness.run(harness, input, options?)` | The one runner, bound to the caller: resolve, floor, render, call, parse, repair, guard, meter, `harness_runs` row |
| `ctx.harness.stream(harness, input, options?)` | The same run, streamed through `onDelta` |
| `resolveHarnessModel(spec)` | The free probe: `{ model, step }` or null. Labels the run without spending a turn |
| `HarnessResult` | What comes back: `value`, `model`, `step`, `answered`, `refused`, `repairs`, `escalate` |
| `ModelChainStep` | `'pin' \| 'slot' \| 'role' \| 'utility' \| 'env' \| 'preferred' \| 'first-routable'`: which step answered |
| `ResolvedHarnessModel` | `{ model, step }`: what the probe returns |

### The free `runHarness`

`runHarness(harness, input, ctx)` is still exported for apps written before `ctx.harness`. It runs
what you hand it: the `caller`, the pin and the `ledger` are your word, and nothing is checked
(the gateway still keeps the `caller` inside your app's namespace, below). Two things changed:

- **`deps` is gone.** It is no longer part of `RunContext`, and if you pass it anyway it is dropped
  with a warning. Test with `runHarnessOffline` instead.
- **Pinning an agent logs a warning**, because nothing here checks that the caller may use that
  agent.

Move to `ctx.harness.run`. It needs no `caller` and no `ledger.source`.

## Where the calls go — one gateway, your app's key

Every model call the runner makes for your app goes to the Talaria gateway, the same one every
agent and API key uses. Your code never holds a key, a provider URL or a transport:

- **Your app has its own gateway key**, `app:<slug>`. The host asks Talaria for it the first time
  your app makes a call, keeps it in memory, and never hands it to app code. Disabling or
  uninstalling the app revokes it; enabling it again mints a new one.
- **The host knows which app is calling.** Calls made from your `server.ts`, from an MCP tool
  handler, or from a fitness replay of your fixtures run as your app. A call made outside all of
  those (module top-level code, say) is refused: there is no app to charge it to.
- **Spend is booked as your app's.** Each call is one ledger row with source `app`, named by the
  run's `caller` kept inside your app's namespace: `app:<slug>` or `app:<slug>:…` is kept as
  written, and any other caller is prefixed with `app:<slug>:`. The provider's own cost is
  recorded when it reports one; a reply without usage is still booked, from an estimate.
- **Your app has a budget.** An admin sets it under Admin → Settings → LLM budgets for the caller
  `app:<slug>`; it counts every row under your app's namespace. Over it, a run fails with the
  gateway's sentence (`"app:<slug>" has reached its LLM budget…`) in `error`, and no call is
  made.
- **A harness pinned to a fleet agent** runs one turn on that agent. Talaria records the turn as
  your app's, so every call the agent makes for it is booked once, as `app` spend.
- **The gateway does not guard your replies a second time.** The runner's guard pass, with your
  harness's `guard.rules`, is the one of record.

A fitness replay of your fixtures is real spend on the same key, stamped
`app:<slug>:fitness:<harness id>`, and the token counts the fitness page shows for it are the
provider's own.

