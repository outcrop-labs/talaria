# Activity harnesses — model calls Talaria runs for you

A harness is a **declared** model call: a prompt, an output contract, a model chain, a
failure policy. You declare one with `defineHarness(...)` and run it through the one runner,
`runHarness` — you never touch a transport, a key, or a retry.

> **Put one harness in each file under `harnesses/`.** Default-export it from
> `apps/<slug>/harnesses/<name>.ts`. While the app is enabled, the harness is checked and
> registered, gets a column in **Admin → Models → Fitness**, and the fitness sweep scores
> candidate models on its `evals` ([Registration](#registration--a-column-in-the-fitness-matrix)).
> In production your `server.ts` still runs it:
> [the bridge pattern](#the-bridge-pattern--running-one-from-your-server) below.

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
| `model` | A `ModelSpec`: `pin` and/or `role` and a `chain` — never a model id |
| `render` | Input → `Message[]`. **The only thing you write by hand** |
| `output` | `text` (`clean`/`verify`) or `json` (`schema`, `repair`, `verify`) |
| `onFailure` | `'null'` \| `'throw'` \| `{ fallback }` \| `{ escalate: true }` — what a failure means |
| `widen?` | Capability-gated prompt widening; `render`'s `ctx.widened` says which branch |
| `guard?` | `{ rules?, redact? }` — which guardrail rules apply |
| `tools?` | A `ToolPolicy` if the model may use its own tools this turn |
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

To run fixtures before you ship, use `runHarness` with a scripted `deps.transport` and no-op
recorders, then call the fixture's `check` on the value. `ui/src/sdk/server.test.ts` does exactly
that with the example above.

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

## The bridge pattern — running one from your server

This is how an app harness runs in production: your `server.ts` imports it and invokes it directly —
run the harness when a model chain is routable, fall back to your own deterministic answer
when it isn't, and journal which one answered. `@/server/harness/run` does not resolve from
app code **by design** — the SDK is the only road in:

```ts
import { defineAppServer, json, resolveHarnessModel, runHarness } from '@talaria/sdk/server'
import triage from './harnesses/triage'

export default defineAppServer({
  async fetch(request, ctx) {
    if (ctx.path !== 'triage' || request.method !== 'POST')
      return json({ error: 'not found' }, { status: 404 })
    const { id, subject, body } = await request.json()

    // Free probe — no model call: nothing routable means answer it yourself.
    if (!(await resolveHarnessModel(triage.model)))
      return json({ severity: null, reason: '', source: 'fallback' })

    const run = await runHarness(triage, { subject, body }, {
      caller: `app:${ctx.app}`,
      // Booked on the spend row when it is a uuid; any other id is left off
      // the row, never the row itself.
      ledger: { refId: id },
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

| export | role in the bridge |
| :--- | :--- |
| `resolveHarnessModel(spec)` | The free probe: `{ model, step }` or null — labels the run without spending a turn |
| `runHarness(harness, input, ctx)` | The one runner: resolve, floor, render, call, parse, repair, guard, meter, `harness_runs` row — the same runner and accounting the platform's own TS harnesses get |
| `RunContext` | `{ caller, userId?, model?, step?, tier?, effort?, ledger?, signal?, deps? }` — annotate a context built apart from the call; `deps` is the test seam (a scripted transport, no-op recorders) |
| `RunLedger` | `{ source?, refId?, taskId? }` — where the turn's spend belongs. `refId` and `taskId` reach the spend row when they are uuids; an app's spend is always booked with source `app` ([below](#where-the-calls-go--one-gateway-your-apps-key)) |
| `HarnessResult` | What comes back — `value`, `model`, `step`, `answered`, `refused`, `repairs`, `escalate` |
| `ModelChainStep` | `'pin' \| 'role' \| 'utility' \| 'env' \| 'preferred' \| 'first-routable'` — which step answered |
| `ResolvedHarnessModel` | `{ model, step }` — what the probe returns |

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

