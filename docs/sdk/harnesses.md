# Activity harnesses — model calls Talaria runs for you

A harness is a **declared** model call: a prompt, an output contract, a model chain, a
failure policy. `apps/<slug>/harnesses/*.ts` default-exports one `defineHarness(...)` per
file. The host merges them into the activity registry (builtin < app-shipped < admin-custom,
by id) and runs them through the one runner — you never touch a transport, a key, or a
retry.

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
    note: 'Runs on any model; a weak one grades more coarsely and the queue stays usable.',
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
| `id` / `label` / `job` | Registry identity; `job` is the one line Admin shows |
| `requires` | Capabilities the model must have — the fitness suite scores against this |
| `floor` | `{ capabilities, refuseBelow, note }` — refuse weak models, or run anyway? |
| `model` | A `ModelSpec`: `pin` and/or `role` and a `chain` — never a model id |
| `render` | Input → `Message[]`. **The only thing you write by hand** |
| `output` | `text` (`clean`/`verify`) or `json` (`schema`, `repair`, `verify`) |
| `onFailure` | `'null'` \| `'throw'` \| `{ fallback }` \| `{ escalate: true }` — what a failure means |
| `widen?` | Capability-gated prompt widening; `render`'s `ctx.widened` says which branch |
| `guard?` | `{ rules?, redact? }` — which guardrail rules apply |
| `tools?` | A `ToolPolicy` if the model may use its own tools this turn |
| `evals?` | Fixtures — your column in the org's model-fitness matrix |

`RenderContext` also carries `model` (name the model in small-model prompts) — it is not an
invitation to branch on model ids; that is what capabilities are for.

## Evals

Each `EvalCase` is `{ name, input, check, band? }`. `check(value, ctx)` returns `null`
(pass), a string (fail the **model**), or `{ gap }` (fail the **fixture**). Bands are
`'easy' | 'standard' | 'hard'`: easy is the floor for usable-at-all, standard is the job.

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

## The bridge pattern — running one live from your server

The registry runs your harness on your behalf, but your `server.ts` may also invoke one
directly: run the harness when a model chain is routable, fall back to your own
deterministic answer when it isn't, and journal which one answered. `@/server/harness/run`
does not resolve from app code **by design** — the SDK is the only road in:

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
| `runHarness(harness, input, ctx)` | The one runner: resolve, floor, render, call, parse, repair, guard, meter, `harness_runs` row — byte-identical accounting to the registry's |
| `RunContext` | `{ caller, userId?, model?, tier?, ledger?, signal? }` — annotate a context built apart from the call |
| `RunLedger` | `{ source?, refId?, taskId? }` — where the turn's spend belongs |
| `HarnessResult` | What comes back — `value`, `model`, `step`, `answered`, `refused`, `repairs`, `escalate` |
| `ModelChainStep` | `'pin' \| 'role' \| 'utility' \| 'env' \| 'preferred' \| 'first-routable'` — which step answered |
| `ResolvedHarnessModel` | `{ model, step }` — what the probe returns |
