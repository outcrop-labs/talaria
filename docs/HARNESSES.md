# Harnesses — one way to reach a model

Talaria has to be **decent on a 7-14B self-hosted model and excellent on a frontier one**, and an
admin has to be able to tell which, per role, from the UI. Everything in this document exists to make
those two sentences compatible.

Before the harness layer there were five ways to reach a model (`proxyChat`, `completeViaGateway`,
`buildUpstream` + `fetchUpstream`, the `/api/llm` route, and one feature's private `requestJson*`
pair), and every place that wanted structured output picked one and then wrote its own prompt, its
own parser, its own fallback chain and its own failure behavior. The 2026-08-06 audit
([`AUDIT-HARNESS-2026-08-06.md`](./AUDIT-HARNESS-2026-08-06.md)) counted what that arrangement had
produced:

- **six** structured-output extractors, three of which failed by execution on ordinary small-model
  output (a fenced object followed by prose containing a `{placeholder}`; a preamble then two
  objects; an object then a bulleted list with a brace in it);
- **six** verbatim copies of the model-fallback chain, plus `'pl-main'` spelled out as a literal at
  seven sites — on an install that never named an endpoint `pl-main`, those subsystems silently
  no-op;
- **three** of the highest-stakes model paths (agent work sessions, research synthesis, Muse
  drafting) running with no guardrail at all, because guards were wired per call site;
- **zero** retries on a malformed structured reply, anywhere.

None of that hurts much on a frontier model, which is why it shipped. All of it hurts on a 14B local
one. And the failure mode was never an error: it was a `return null` a fire-and-forget caller
swallowed, so a bad model presented as a feature that quietly stopped working — a different silent
behavior at each of the six sites (the judge escalated to a human, the blurb writer re-burned the
same batch every ten minutes forever, Muse's buttons just did nothing).

Every one of those is a property of the *arrangement*, not of any one site. So there is now one
runner, and a harness declares rather than does.

## What a harness is

A **harness** is a declarative object: a prompt renderer, an output contract, a model policy, a
failure policy, and a set of eval fixtures. It never chooses a transport, a model, a parser or a
failure behavior — it *declares* them, and `runHarness` honors the declaration.

| File | What it owns |
| --- | --- |
| `server/harness/define.ts` | The contract. Types and one identity function — no DB, no gateway, no settings, so a definition can be built and enumerated without booting Talaria. |
| `server/harness/run.ts` | `runHarness` / `runHarnessStreamed`. The only code that talks to a model. |
| `server/harness/json.ts` | The one structured-output parser and the one repair wording. |
| `server/harness/text.ts` | The one first-line extractor for text harnesses. |
| `server/harness/model.ts` | The resolution chain, expressed once. |
| `server/harness/capability.ts` | What a model can actually do, and who says so. |
| `server/harness/transport.ts` | The gateway and fleet-persona transports (blocking + streaming), the request that reaches them, and the refusals a transport raises rather than dropping a field it cannot honor. |
| `server/harness/registry.ts` | The 23 shipped harnesses, merged builtin < app-shipped < admin-custom. |
| `server/harness/defs/*.ts` | The definitions themselves — 32 harnesses, 289 eval fixtures. |
| `server/harness/recorded.ts` | Run any harness against written-down replies: no gateway, no fleet, no DB, no clock. |

**One chokepoint, and CI holds it.** `node scripts/check-invariants.mjs` fails the build on a call to
`proxyChat` / `completeViaGateway` / `buildUpstream` / `fetchUpstream` outside `server/harness/` and
the two gateway modules that define them (`hand-written-harness`). Four census entries remain and
none of them is debt: the public `/api/llm/v1/chat/completions` pass-through proxy, and three live
persona conversations (`routes/api/chat.ts`, `chat-persist.ts`, `channel-replies.ts`) where the
messages are the human's and there is no prompt of Talaria's to declare. A fifth match is a
regression, not a backlog item.

If what you need is a runner *capability*, ask for it in `run.ts`. Five files once hand-wrote their
own persona transport because the runner could not pass a model's own tools through, stream, carry
ledger attribution or route a persona tier — and three of those five silently dropped `temperature`
and `jsonMode` on the way, reintroducing the exact class of bug the layer exists to end. All four
slots exist now (`def.tools`, `runHarnessStreamed`, `ctx.ledger`, `ctx.tier`) and all five shims are
deleted.

## What the runner does, in order

```ts
const result = await runHarness(titlerHarness, { kind: 'chat', text }, { caller: 'platform:titler' })
```

1. **Resolve the model** from `def.model` — and record *which chain step won*, because a subsystem
   limping along on `first-routable` for a month is a real finding that used to be invisible.
   `ctx.model` pins and skips resolution entirely.
2. **Check the floor.** Known-missing capabilities are intersected with `floor.capabilities`; the run
   is refused only if `floor.refuseBelow` is true *and* the evidence is a probe or a declared fact,
   never something the gateway learned from a single 400.
3. **Decide widening.** `def.widen.requires`, all known-true from a probe, or the narrow surface.
4. **Render**, with one `RenderContext` (`{ widened, model }`) that `verify` later sees too.
5. **Call**, through the transport `pickTransport` chose — gateway or fleet persona. Structured runs
   carry the JSON anchor in the prompt *and* `response_format` where the model is not known to refuse
   it; the two are belt and braces, not alternatives.
6. **Parse, then repair.** One extractor, one schema check, one `verify`, then one re-ask with the
   parser's own sentence.
7. **Guard**, with an honest `Available` for the transport that actually ran, then redact the value
   if the harness persists its output.
8. **Apply `onFailure`.**
9. **Write the `harness_runs` row** — harness, model, chain step, widened, repairs, schema-valid,
   latency, findings, caller, error. Every exit writes one, including the exits that never reached a
   model.

Nothing escapes as an exception except `onFailure: 'throw'`. `render`, `clean`, `verify` and `ground`
are harness-author code meeting model output; a throw out of any of them is a failed contract, not an
escaped error.

## The contract, field by field

The contract is `server/harness/define.ts`. The smallest harness in the product, abridged
(`server/harness/defs/titler.ts`), is the shape of one:

```ts
export const titlerHarness = defineHarness<TitlerInput, string>({
  id: 'titler',
  label: 'Titler',
  job: 'Names things as they take shape: chats and plans after their first exchange, research runs from their question.',
  requires: ['instruction-following'],
  floor: { capabilities: [], refuseBelow: false, note: 'Runs on any model. …' },
  model: { pin: 'titler' },
  render: (input) => [{ role: 'system', content: PROMPT[input.kind] }, { role: 'user', content: clip(input.text) }],
  output: { kind: 'text', clean: cleanTitle },
  onFailure: 'null',
  guard: { rules: ['secret_leak', 'pii_leak'], redact: true },
  temperature: 0.3,
  evals: [ /* … */ ],
})
```

### `requires: Capability[]`

What the job leans on. **`requires` never blocks anything.** It is what the fitness matrix scores
against and what the admin UI shows next to the model picker, so a weakness is visible rather than
enforced. The nine capabilities are `json`, `json-strict`, `tools`, `tool-select`, `search`,
`vision`, `long-context`, `code`, `instruction-following`.

### `floor: RoleFloor` — and why the floor is per role

`{ capabilities, refuseBelow, note }`. **The floor is per role, not global, and that is the locked
design decision**: "decent on a 14B, excellent on a frontier model" is only coherent if each job says
for itself what it cannot do without.

The titler, the summarizer and the librarian declare an empty floor. They have to work on whatever
the self-host has, and a titler that *refuses* leaves every conversation wearing a truncated first
message forever — a worse product than a clumsy name.

Two harnesses refuse today, and both are cases where degrading silently is worse than stopping:

- **`judge`** (`json`, `json-strict`, `instruction-following`) — a judge that degrades returns
  verdicts that are trusted and wrong, and its verdicts move tickets. With `mode: 'enforcing'` (the
  default) every escalation notifies the board's editors, so a weak judge model reads as a
  notification storm rather than as a review. Below the floor, the ticket simply waits for the human
  reviewer it was already waiting for.
- **`research-search`** (`search`) — a model with no live web search does not fail. It answers from
  memory, with no sources, in exactly the same confident voice, and the brief reads like a researched
  one.

`capabilities` is the non-negotiable **subset** of `requires`, and it must be **empty unless
`refuseBelow` is true**. The runner only ever reads it when the harness refuses, so a populated list
on a non-refusing floor is inert while reading to the next author as a hard requirement — which is
how the port arrived with two spellings of "needs JSON, runs anyway". `registry.test.ts` enforces
both rules.

`note` is one sentence written for the admin choosing a model, not for the developer reading the
file. It is shown next to the model picker and printed in the refusal.

### `model: ModelSpec`

`{ pin?, role?, chain?, userId? }`. The default chain is
`['pin', 'role', 'utility', 'env', 'first-routable']` — the eight lines that had seven spellings
across the tree, four of them verbatim copies and three with local variations. A model only wins
**while it still routes on the gateway**, which is the inherited contract that stops a deleted model
from silently breaking a subsystem. `'pl-main'` survives as a named preference inside the last-resort
step and nowhere else; no harness spells it again.

Two things worth knowing before you write one:

- **`chain: []` is a declaration, not an oversight.** It is right for every harness whose model comes
  from the *subject* of the call — the owner's assistant, the agent on the ticket, the agent in the
  channel or the plan. Those callers pass `ctx.model`, so the chain is never consulted; `[]` says
  what should happen if a caller ever forgets ("nothing, loudly") instead of letting a work-session
  turn quietly run on the org's utility model and be filed as the assigned agent's work.
- **Never write `role: 'utility'`.** It resolves the same model through the same allowlist gate as
  the default chain's `'utility'` step and differs only in the `chain_step` recorded on the run, so
  two identically-resolving harnesses would report differently to the fitness page. `role` is for a
  harness with a role of its own.

The member model allowlist (org policy: which models a non-admin may be handed) applies to the steps
that hand over a user's own choice or the user-visible catalog — `preferred`, `role`, `utility`,
`first-routable` — and deliberately not to `pin` or `env`, which are org policy themselves.

### `render: (input, ctx) => Message[]`

The only thing a harness author writes by hand. `ctx` is `{ widened, model }`. `model` is there so a
render can name the model in its own prompt or size its context — **not** an invitation to branch on
model ids; that is what capabilities are for.

`Message.content` is a string across the whole tree. That is a re-decided constraint, not an
oversight: widening it to the OpenAI content-parts union would have to land in `Message`, both
payload builders, `groundingTextOf`, `extractToolRecord`, `lastUserMessage`, `anchorJson`, both token
estimates and 23 renders at once, and a half-widened union would report `[object Object]` into the
ledger. The `vision` probe skips and says so, rather than recording a fact it could not measure.

### `output` — text vs json, and `verify`

```ts
output: { kind: 'text'; clean?: (raw: string) => O | null; verify?: Verify<I, O> }
      | { kind: 'json'; schema: z.ZodType<O>; repair?: number; verify?: Verify<I, O> }
```

`kind: 'json'` puts the runner in structured mode. `kind: 'text'` with no `clean` means `O` is
`string` by construction; `clean` is where a text harness narrows and returns `null` to fail the
contract. Unwrapping a reply the model wrapped is `firstMeaningfulLine` (`harness/text.ts`), shared by
the titler and the summarizer — which shipped the same eight lines with a one-character difference,
and between them stored a bare ` ``` `, a trailing `**` and a "Here's the summary:" lead-in as if
those were answers. A `clean` should hold what is true of *its* output and nothing else.

`verify` is the other half of the contract, and the half neither a schema nor a `clean` can express —
see the next section.

### `onFailure`

What a failure *means here*, stated once and in public, because before this existed each site
answered it differently and in silence.

| Value | Meaning |
| --- | --- |
| `'null'` | The caller keeps what it had (titler, summarizer). |
| `'throw'` | The caller must handle it. |
| `{ fallback: O }` | A declared safe value — a default verdict, an empty list. |
| `{ escalate: true }` | A human decides. The runner sets `escalate` on the result; the caller raises it, because only the caller knows who to tell. |

`escalate` is a **flag**, not a phrase in the error string: a caller that string-matches an error
message is a caller that stops escalating the day somebody rewords it.

`'throw'` means **any** failure to produce a value — nothing routed, the floor refused, `render`
threw, the transport died, the contract failed. The other three stay contract-scoped, and that
asymmetry is deliberate: `{ fallback }` on a gateway outage would hand outreach its "nothing to
surface" token on every sweep, and `{ escalate: true }` on an unreachable model would notify every
board editor about every ticket for as long as the gateway is down — the notification storm this
whole audit is about. A caller that wants either of those on an unreachable model reads
`result.answered` and says so on purpose.

### `guard`

`{ rules?: string[]; redact?: boolean }`. `rules` narrows the guardrail registry to the ids that make
sense for this output; `redact` strips credentials and PII out of the **value** the runner returns,
for harnesses whose output is persisted.

Two sharp edges, both locked by `registry.test.ts`:

- **A misspelled rule id disables every rule for that harness**, not one. `narrowGuardConfig` turns a
  rule on only when the harness names it, so a typo leaves a `guard` block in the file that reads as
  protection and provides none.
- **Omitting the block entirely runs every enabled rule**, which is the opposite failure and just as
  bad. That is what put the judge on `zero_tool_claim` and `fabricated_outage` — rules structurally
  wrong for a verdict, which *describes* claimed work rather than doing any — and inflated
  `guard_findings.model`, the per-model confabulation rate the fitness page reads, for whichever
  model the admin had chosen to judge with. All 32 harnesses name their rules. A harness that
  genuinely wants all of them says so by listing them.

Redaction runs on the raw reply and then **re-applies the whole contract, `verify` included**, so a
redacted value is a value that still satisfies the schema. If the redacted form no longer parses, the
harness gets nothing — handing back a value with a live credential because the clean version failed
to validate would be the worst of both.

### `widen`

`{ requires: Capability[]; note: string }`. Set it and `render` is called with `widened: true` only
when every capability listed is **known-true from a probe** for the resolved model. Both branches
must be real answers: `widened: false` is the product working, not a degraded mode with an apology in
it. Twelve of the 32 harnesses widen; the titler deliberately does not, because a wider prompt would
only buy a longer name.

### `ground`

`(input) => Grounding | null` — the turn's real tool record, from the harness's own input.
`ungrounded_ref` ("cites a link/UUID that wasn't in any tool result") is the
highest-value rule in `guardrails.ts` and it **could not fire from any harness**, by construction: a
harness turn contains no tool messages, so the record the runner derives has no backing tools and the
rule declined every time. Research's synthesis stage *is* handed the search hits and the numbered
source registry — those are the tool results for that turn — and `ground` is how it says so.

Honesty is expressible here; optimism is not the default. No hook, a hook returning `null`, and a
hook returning an **empty tool list** all mean the same thing: keep whatever `Available` the
transport earned, and let the rules skip. Claiming a grounded turn with no backing tool would turn "we
cannot check this" into "we checked and it is fine", which is the one direction a guard must never
move.

### The rest

`temperature`, `tools` (`'none'` by default — enforced at the protocol level, not left to the prompt;
`'own'` is for the three turns whose whole feature *is* the tool loop), `toolDefs` (tools this
harness offers the model, and a different question from `tools` — its only declarers today are the
`tools` and `tool-select` probes, and a harness that declares it must resolve to a **gateway** model,
because all three other transports refuse the call rather than answer a question the model was never
asked), `holdMs` (how long a
persona transport may hold for an agent that is restarting — ten minutes for a work session, thirty
seconds for a panel a person is watching), and `evals`.

## The small-model story

Four mechanisms, and each one closes a failure that was verified by execution rather than reasoned
about.

**The balanced-brace scanner.** `extractJson` walks the text counting braces, aware of string
literals and escapes, and yields every complete `{`/`[`-rooted span — **fenced blocks first**, because
a fence is the model explicitly saying "this is the payload". Without that ordering, "According to
[1], here is the list: ```["a","b"]```" extracts `[1]`, a perfectly valid JSON array that happens to be
a citation marker. Candidates that do not parse are skipped and the scan continues, which is what
turns "the model rambled first" from a failure into a non-event. `relaxJson` then tolerates a trailing
comma, curly quotes used as delimiters, bare `NaN` / `Infinity` / `undefined`, and a raw newline
inside a string.

What it deliberately does **not** do is guess at truncation. A value whose braces never close is a
failure, reported as one, with a repair instruction attached. Inventing the missing tail is how a
harness returns a confidently wrong answer instead of an error.

**The repair turn.** One re-ask by default (`output.repair`), carrying `parseJson`'s own sentence:
"missing required field 'verdict'", "field 'plan.steps[2].title' should be string, got number". At
most three problems are named — a small model handed eleven fixes rewrites the whole thing and
reintroduces the first one. For a frontier model this is worth roughly nothing; for a 14B model a
single repair round-trip converts most structured-output failures, which is why the audit called it
the highest-leverage item in the document.

Three limits, each of them a decision:

- **The repair turn is the one place the runner puts model output back into a model's context**, so
  the reply goes through the gate-safe rules first. A flagged reply is not repaired; it fails. The
  repair prompt carries the parser error and *nothing else* — never the finding, never its `snippet`,
  which is a verbatim excerpt of the flagged content.
- **A streamed run never repairs.** The first answer already reached the screen; repairing would
  stream one document and hand back another. The failure is recorded honestly instead, which is also
  the number the fitness page needs.
- **A text harness never repairs**, including on a `verify` failure. The one repair wording lives in
  `json.ts` and ends "send the corrected JSON value only", which is nonsense to a titler. A text
  harness that needs a repair turn needs a second wording first, not a branch in the runner.

**`verify` — the half a schema cannot state.** A schema is a module constant. It is built once, at
import time, and **it cannot see the run's input**, so every harness whose correctness is a *relation*
between what was asked and what came back had no way to say so — and the runner recorded
`schemaValid: true` for values the caller then threw away. Four shipped bugs were that one defect in
different clothes:

| Harness | What the schema could not say |
| --- | --- |
| `blurb-writer` | `z.record(z.string(), z.string())` cannot constrain the **keys**. A model that tidied `qwen3-14b` into `Qwen3 14B` passed the schema, wrote zero blurbs, reported a 100% contract rate, and came back around on the identical batch every ten minutes forever. |
| `channel-plan` | The elements must be **tickets from the transcript**, not titles the model invented. |
| `muse:ticket` | A date must be one the **write path** accepts (`z.string()` here against `z.string().datetime()` on the route), so the repair turn could never fire on the likeliest small-model mistake. |
| redaction | A value that still parses after being cut in half. |

The offline eval fixtures already knew — `blurb-writer`'s own fixture rejects invented ids — so the
fixtures and the `harness_runs.schema_valid` column disagreed, and the production one was the
optimistic liar. `schema_valid` is the observed half of the fitness matrix; a column that says a model
held a contract it did not hold makes the matrix worth less than nothing.

A `verify` returns `null` when the value is usable, or **one plain sentence** naming the problem. That
sentence is fed straight back to the model as the repair instruction, so write it as an instruction to
a model and not as a note to a developer: *"the keys must be the model ids exactly as given - 'Qwen3
14B' is not one of them"* repairs; *"invariant violated in blurbWriter"* does not. A verify failure is
a contract failure in every sense the runner has: same repair loop, same counter, `schemaValid: false`,
and an honest `harness_runs` row. It receives the same `RenderContext` the prompt was built from,
because on a widened surface the contract itself changes — `inbox-command` offers a probed model the
item's whole action list and a regex-bound one a single id, and "did it stay inside what it was
offered" is unanswerable without that.

Keep it cheap and pure: it runs on every attempt of every run, including the redaction re-check, and
`define.ts` imports no database by construction. A check that needs to ask the database whether an id
exists belongs to the caller, not to the contract.

## The capability model

`server/harness/capability.ts` is the type Talaria never had, and two live bugs shared its absence as
their root cause: role assignments were validated for *routability* and nothing else (so an admin
could point `research-recon` at a model with no web search and get a confident, uncited brief), and
the gateway's parameter learner would strip `response_format` on a 400 and let the call **succeed**,
returning free prose to a caller that was about to run a JSON parser.

A **capability fact** is `{ value, source, at, detail?, score? }`, keyed `endpoint:model` — capability
is a property of the endpoint serving the model, not of the name. One id behind two providers (a
quantized local build vs. the vendor's own API) genuinely differs in what it can hold, and a fact
learned from one must never be credited to the other. Three writers, three provenances:

| Source | Written by | Expires |
| --- | --- | --- |
| `probe` | The tier-1 probe suite — Talaria's own measurement. | Never; a probe fact stands until someone re-measures. |
| `learned` | `llm-gateway.ts`, from what an upstream 400 said. Only ever `false`. | 30 days (`LEARNED_TTL_MS`) — the release valve on what was a one-way ratchet. |
| `declared` | An admin, or a model catalog. | Never; it is a human's word until the human changes it. |

### Unknown is not false at the floor. Unknown does not widen.

Those look inconsistent. They are the same rule asked from two ends, and in both cases the safe
direction is the same one: **keep running, on the deterministic surface.**

- At the floor the question is *"is this model known to be **unable**?"* Only a fact that positively
  says "no" counts as missing. Talaria cannot refuse to work until an admin gets around to running a
  benchmark, and a fresh self-host has probed nothing at all.
- At the widen gate the question is *"is this model known to be **able**?"* A model nobody has probed
  gets the narrow prompt, which works everywhere, rather than the wide one, which does not.

Unknown is the answer to neither question, and it lands on the same behavior both times.

### Widening requires a measured fact

The floor accepts a `declared` fact as grounds to refuse — a human or a catalog saying "this model
cannot do JSON" is evidence enough to stop, and refusing is recoverable in one click. **Widening
requires `source: 'probe'`**, because widening is the direction that hands a model more authority:
`inbox-command` widened is a model choosing which action to take on somebody's ticket instead of
confirming the one a regex already chose. The evidence for that has to be Talaria's own measurement —
four tools offered, four correct picks observed — and not a line in a vendor's model card. The moment
anything imports a catalog as `declared: true`, a marketing claim would otherwise widen the Inbox
across every install that synced it.

Refusing is held to the same standard from the other side: a **learned** fact never refuses a run. The
gateway writes `json: false` the first time an upstream 400s on `response_format`, which is evidence
about one parameter on one endpoint, not a measurement of whether the model can produce JSON. Counted
as a floor, one 400 turned the judge off for every board for the full learned TTL, with no
notification and no admin surface that said so. It now shapes the *request* (the runner stops sending
`response_format`, and the prompt anchor carries the ask) and never the decision to run.

### Pools and personas

A bare model id may be served by a **pool**, and the runner cannot know which member will take the
call without advancing the round-robin cursor. So both questions are answered **unanimously**: a
capability counts as missing only if every member says missing, and counts as earned only if every
member says earned.

A fleet persona is not a catalog model — `routingFor` answers with no endpoints for one — so
`harness/persona.ts` resolves the endpoint and upstream model behind it (its agent version's
`config.main`, or the alias for the tier being called) and the persona inherits that probe. Without
it, `widened` was structurally always false on personas, which is exactly where the marquee widening
case runs: the Inbox command harness runs on the owner's personal assistant.

## How to add a harness

A worked example: a "release-notes" harness that turns a list of merged PR titles into a short
changelog entry.

**1. Write the definition** in `ui/src/server/harness/defs/release-notes.ts`. Keep it pure — types,
prompts, a `clean`/`schema`, and fixtures. It must be importable without a database.

```ts
import { z } from 'zod'
import { defineHarness } from '../define'

export interface ReleaseNotesInput {
  version: string
  /** PR titles, newest first. */
  changes: string[]
}

const NOTES = z.object({
  headline: z.string(),
  bullets: z.array(z.string()).min(1).max(8),
})
export type ReleaseNotes = z.infer<typeof NOTES>

export const releaseNotesHarness = defineHarness<ReleaseNotesInput, ReleaseNotes>({
  id: 'release-notes',
  label: 'Release notes',
  job: 'Turns a list of merged changes into a short, user-facing changelog entry.',

  // What the job leans on. Never blocks; this is the fitness matrix's row.
  requires: ['json', 'instruction-following'],

  // No floor: a clumsy changelog is better than none, and the caller keeps
  // whatever it had when the contract fails.
  floor: {
    capabilities: [],
    refuseBelow: false,
    note: 'Runs on any model. A weak one writes flatter bullets; nothing downstream parses them.',
  },

  // No pin: the default chain runs the Utility role, then the env default, then
  // the first routable model. Add a `pin` when an admin should be able to assign
  // this harness its own model (see below).
  model: {},

  render: (input) => [
    {
      role: 'system',
      content:
        'Write a changelog entry from the merged changes below. One headline of at most 8 words, ' +
        'then 1-8 bullets in the user\'s language, not the committer\'s. Do not invent changes.',
    },
    { role: 'user', content: `Version ${input.version}\n\n${input.changes.join('\n')}` },
  ],

  output: {
    kind: 'json',
    schema: NOTES,
    // THE HALF THE SCHEMA CANNOT STATE: there can be no more bullets than there
    // were changes, and each one has to share a distinctive word with one of
    // them. Written as an instruction to the MODEL, because it becomes one.
    verify: (value, input) => {
      if (value.bullets.length > input.changes.length) {
        return `you wrote ${value.bullets.length} bullets for ${input.changes.length} changes - write at most one bullet per change`
      }
      const words = (s: string) => new Set(s.toLowerCase().match(/[a-z]{5,}/g) ?? [])
      const known = input.changes.map(words)
      const invented = value.bullets.find((b) => {
        const w = words(b)
        return !known.some((k) => [...w].some((token) => k.has(token)))
      })
      return invented ? `"${invented}" is not one of the changes you were given - write only about the listed changes` : null
    },
  },

  // The caller already has a draft entry; null means "keep it".
  onFailure: 'null',

  // Named, not omitted: an omitted block runs EVERY enabled rule, and a
  // changelog legitimately describes completed work, which is what
  // zero_tool_claim matches.
  guard: { rules: ['secret_leak', 'pii_leak'], redact: true },
  temperature: 0.2,

  evals: [ /* see the next section */ ],
})
```

**2. Register it** in `server/harness/registry.ts`'s `BUILTINS`. This is the last step of a port, not
a follow-up: a harness that is not in `BUILTINS` is invisible in the two ways that matter most — the
fitness suite cannot replay its fixtures, so every assertion its author wrote is dead code, and the
admin panel cannot show its floor. One phase of this project landed nine definitions with 32 fixtures
between them and registered none of them.

**3. Call it** from the feature, and handle the failure policy you declared:

```ts
const result = await runHarness(releaseNotesHarness, { version, changes }, { caller: 'platform:release-notes' })
if (!result.value) return keepExistingDraft()   // onFailure: 'null' — the caller keeps what it had
```

**4. Test it against recorded replies.** `server/harness/recorded.ts` runs any harness with no
gateway, no fleet, no database and no clock — the transport is scripted, but the parser, the guard
rules and the persona resolver are real, so an assertion about the runner stays an assertion about the
runner. Include at least one reply in the shape a 14B model actually emits (a preamble, then the
object), so the extractor and the repair turn are exercised rather than assumed.

**5. Check the invariants.** `node scripts/check-invariants.mjs` and
`cd ui && ./node_modules/.bin/vitest run`. `registry.test.ts` will fail the build if the harness names
a guard rule that does not exist, leaves the guard block off, declares a floor it never enforces,
refuses on a capability it does not require, spells the Utility fallback as `role: 'utility'`, widens
without a note, or ships no eval fixtures.

**Shipping one from an app** is the same contract in a different home: `apps/<slug>/harnesses/*.ts`,
one harness per file, default-exported, loaded for enabled apps only and checked structurally before
it joins the registry — a broken app is a logged skip, never an empty admin page. See
[`SDK.md`](./SDK.md). The third layer, admin-custom, is deliberately empty: a workbench harness is
declarative and can live as a JSON row, but an activity harness carries code (`render`, `verify`,
`check`), and Talaria does not run code out of a database row. What an admin *can* customize is the
model each harness runs on.

**If an admin should be able to assign this harness a model**, it needs a `PLATFORM_AGENTS` entry and
`model.pin` pointing at it; `registry.test.ts` locks the harness↔platform-agent correspondence in both
directions, including the two places the lists deliberately do not line up (harnesses whose model
comes from the subject of the call have no pin; the judge has a platform agent but keeps its model in
`judge_config` so the Guard panel and the Platform panel cannot disagree).

## How to write evals

```ts
export interface EvalCase<I, O> {
  name: string
  input: I
  check: (value: O, ctx: EvalContext) => string | null
  band?: 'easy' | 'standard' | 'hard'
}
```

`check` is a plain assertion over the parsed value, not an expected output. **No model judges a
model.** Most harness assertions are string facts — "3-7 words", "no invented status", "never an
`actionId` outside the allowlist" — and a deterministic check keeps the suite fast, cheap, and free of
the who-judges-the-judge regress. Return `null` to pass, or **one line saying what was wrong**; that
line is what an admin reads in the fitness drill-down, so state the observed fact rather than a rule
id ("`5 words — the prompt asks for 3-7`", not "`WORD_COUNT`").

Rules of thumb, all of them learned the hard way:

- **Close over the input.** Build the case with a helper that hands the same input to both the run and
  the check, so a restatement assertion is measured against the real transcript rather than a copy
  that can drift away from it.
- **Assert the relation, not the vibe.** `judge` is the one harness whose correctness cannot be a
  string assertion, and it is scored by *agreement with a labeled fixture set* rather than by a
  meta-judge. Labeled fixtures are cheaper and more honest.
- **A one-sided fixture needs a floor.** Six fixtures across the summarizer, distiller, briefer and
  outreach once asserted only that the answer was not too long, not markdown, not a question and not a
  repeat of the input — every one of which is satisfied by saying almost nothing. Replaying the
  literal string `{"nope": true}` through the whole registry scored six passes. `belowAnswerFloor`
  (`define.ts`) is the fix: state how short is too short to be an answer at all, and — where the input
  has an unmistakable subject — a **set** of alternative words the answer has to have engaged with. A
  set, never a phrase: a fixture only one wording can pass measures our prompt, not the model.
- **Name the safety assertion.** `inbox-command`'s fixture asserting that the model never proposes an
  `actionId` outside the allowlist is the one the whole feature rests on; that same relation is also
  in `output.verify`, so the offline fixture and the production column agree by construction.
- **State the shared assertion once.** Several suites shipped with two fixtures that spelled the same
  four checks in two different orders, one of them omitting a rule — so which fixture you read decided
  what you believed about the model. Every suite now has one `…Problem(value, …)` function carrying
  what is true of *every* answer, and each fixture adds only the part its own input makes checkable.
- **Reference fixtures by NAME in tests, never by index.** `evals?.[0]` silently re-points at a
  different fixture the moment a suite grows, and the failure reads as "the check is wrong" rather
  than "this test is holding the wrong fixture".

### Bands

`band` splits a suite into `easy` / `standard` / `hard`, reported separately by the fitness suite
(`HarnessScore.bandScores`). It defaults to `standard`.

One flat pass rate cannot tell *competent, loses the hard edge cases* from *unreliable on the basics*,
and those are different purchasing decisions: a 70% that is easy 100 / standard 100 / hard 20 is a
fine Utility model, while a 70% that is 70 across all three fails one job in three at random. Bands
also fixed a concrete defect — `muse:ticket` decided the Utility and Muse verdicts from **two**
fixtures, so one failure was 50%, more than 10% under the floor, and a whole model was rejected on a
coin flip. Aim for **8–12 fixtures per harness**, spread across the bands.

### The dry run — measuring what a model DID, not what it said

Three harnesses declare `tools: 'own'` because the tool loop *is* the feature (`work-session`,
`outreach:check-in`, `briefer:chat`), and three more are coding harnesses (`workbench:*`). For those,
"did it *say* it triaged the ticket" is the wrong question; the failure that costs an org a week is a
model that says so having called nothing.

A harness declares `dryRun` and the fitness suite supplies the loop, against an isolated in-memory
world (`server/fitness/toolbox/`):

| Surface | Caller | Tools | World |
| --- | --- | --- | --- |
| `dryRun.tools` (`toolbox/talaria-tools.ts`) | **Hermes agents**, over MCP | Talaria's real MCP toolkit — all 44, names, descriptions **and argument names copied from `mcp/src/index.ts`** and locked by `talaria-tools.sync.test.ts` | boards, tickets, attachments, channels, teammates, the knowledgebase, documents, Google, research runs, board governance |
| `dryRun.workspace` (`toolbox/hermes-tools.ts`) | **Hermes coding harnesses** (Claude Code, Codex, Aider …) | the base coding surface: `list_files`, `read_file`, `search`, `write_file`, `run_tests` | a small repository, per fixture, with the fixture's own pass oracle |
| `def.toolDefs` (`toolbox/native-tools.ts`) | **native platform agents** — a harness in this repo, loop run in-process | whatever the harness puts on the request. Today: one web-search tool, supplied by the org's MCP registry | the live web, through the registered supplier |

**Know which surface a verdict is about.** The first two are the *Hermes* surface: a containerized
persona holding forty-four workspace verbs, whose loop Talaria never sees. The third is what *Talaria
itself* hands a model, and it is one tool long — that shortness is a finding, not a gap in the docs. A
deployment can be good at one and useless at the other: a gateway that strips `tools` from a request
kills every native harness and leaves every persona untouched, because the persona's loop never
travels through that gateway.

Fixtures then assert over `ctx.calls` — the log of what actually happened. `NO_TOOLS` is what every
single-shot harness's fixture receives, so a check reaching for `ctx.calls` sees an honest empty list.

### Every tool must be simulated and exercised — enforced

`scripts/check-invariants.mjs` fails the build when a tool registered in `mcp/src/index.ts` is not

1. **modelled** in `fitness/toolbox/talaria-tools.ts` (real description, real argument names),
2. **backed** by a handler in `fitness/toolbox/sandbox.ts` over the in-memory world, and
3. **exercised** by a sandbox test or a harness's `dryRun.tools` surface.

This is not a style rule. The toolkit reached forty-four tools while the simulator modelled sixteen —
no single commit was wrong, and the result was twenty-eight verbs an org depends on with no simulated
backend, no fixture, and a fitness page reporting confident scores over the third of the surface that
happened to be covered. Eight of those sixteen also carried *invented argument names*
(`comment(body)` for the real `comment(content)`, `add_time(minutes)` for `add_time(seconds)`), so a
model that "called them correctly" in the benchmark got a 400 in production and the benchmark called
it competent.

**Simulate the refusals, not only the happy path.** "Personal assistants only", "no Google account
connected", "that ticket is off the table", "that channel id does not exist" — the failures worth
catching are a model reaching for a tool its identity does not carry, and a model narrating a result
it was refused. A sandbox that only ever says yes measures nothing.

**Offer the discovery tool with the reader.** `list_tickets` takes a `boardId` and `read_channel`
takes a `channelId`; production 404s on a name. A `dryRun.tools` surface with the writer but not the
lister makes a model guess an id and then scores the guess — our gap, charged to the model. This is
the single most common way a fixture ends up measuring the harness instead of the candidate.

The same bar applies to plugin harnesses: a plugin that ships tools ships their simulated backends and
their fixtures. The platform is held to it first, in CI, because a rule the platform exempts itself
from is a rule nobody follows.

Two things are deliberately *not* claimed. The loop is **ours**, not the persona's (production runs
Hermes's), so what it measures is the decision — given these tools and this situation, what did the
model do — rather than end-to-end harness driving. And `run_tests` **does not execute code**: it
applies the fixture's own predicate to the current file contents, because running model-written code
inside a benchmark is a sandbox-escape surface and a flake source. The model cannot tell the
difference; a reader of the verdict should be able to.

## The fitness suite

`server/fitness/` answers, per install: *for each role and each harness in my Talaria, is this model
good enough, and where exactly does it break?* Three tiers, cheapest first, so a bad model is rejected
in seconds rather than dollars. The surface is **Admin → Models → Fitness**
(`routes/app/ModelsFitnessPanel.svelte` over `routes/api/admin.model-fitness.ts`). The route is
plumbing — the admin gate, the query string, the zod body, the audit line, the status code — and
holds no decisions: `fitness/surface.ts` orchestrates, archives and assembles every payload, because
`vitest.config.ts` excludes `src/routes/**` and a decision that lives there cannot be tested.

### Tier 1 — probes (`fitness/probes.ts`)

Nine probes, **22 calls** on a gateway-served candidate (`estimateProbes` prices them before an admin
presses anything). Five of those are the two tool probes, which skip on a fleet persona; the `vision`
probe skips everywhere, so a persona candidate is 17. Cheap calls that establish model-level facts
and write the capability record. This is the
first production writer of `value: true`, and every verdict is asymmetric on purpose: **proof of
presence may be a single verified observation; proof of absence has to come from a check that cannot
fail for an unrelated reason.** Where neither is available the probe writes nothing, and an absent
fact means unknown, which is already safe.

Deterministic scoring only — a parse, a string equality, a clock, an HTTP GET, a `vm` run of
assertions. A probe that *errors* writes nothing at all: a transport down or a gateway restarting is a
fact about the deployment, not about the model. Probes reach the model through `runHarness` with
`ctx.model` pinned, with `missingCapabilities`/`capabilities` stubbed empty (a probe measures the
model, not the record — otherwise one bad 400 could never be re-discovered) and with `recordRun` and
`recordFindings` disabled (synthetic traffic must not move the production numbers it will be compared
against).

`tools` and `tool-select` **skip** on a fleet persona, whose tool loop runs inside the agent container
where Talaria can neither place its tools nor see the call. `vision` skips everywhere while
`Message.content` is a string. A skip is never a `false`.

### Tier 2 — harness conformance (`fitness/evals.ts`)

Replay every registered harness's fixtures through `runHarness` with the candidate pinned. It is a
driver, not a subsystem: `registry.ts` enumerates, `define.ts` supplies the fixtures, `run.ts` already
takes a pin and a transport seam.

Five numbers per harness — `contractRate` (held on the first attempt), `repairRate` (held at all,
cumulative), `taskScore` (the fixture's own `check`), `guardRate`, and latency/cost. **`repairRate` is
the one that matters**: a model at 40% first-pass and 95% after one repair is usable; one at 40/45 is
not, and nothing in Talaria could tell those apart before. That distinction is the entire argument for
the repair turn.

The sweep never re-derives "did the contract hold". It captures the literal `HarnessRunRow` the runner
hands to `recordRun` and scores `row.schemaValid` — two spellings of that predicate is how the offline
fixtures and the production column came to disagree in the first place. The fixture's `check` is scored
separately as `taskScore`, and the two are allowed to disagree; the sweep counts the cases where they
do.

`contractRate` is **not comparable across harnesses** — a titler's non-empty-string check and the
judge's zod-plus-verify are both `true` and mean different things. It is aggregated per harness, which
is the only reading that means anything.

### Tier 3 — adversarial (`fitness/adversarial.ts`)

Opt-in and expensive. Provokes the four failure modes `guardrails.ts` already fires on in production —
zero-tool claims, ungrounded refs, fabricated outages, secret echoes — and scores every generation
with `runGuardrails` over the shipped `RULES` registry. **No new detection logic, ever**, because
`guard_findings` is the observed half of the fitness page and this tier is the benched half; they can
only be shown side by side if the same detector produced both. The honest caveat travels with it: the
rules are lexical, so this tier measures *what the guard can see*, not what the model did.

Each generation is scored twice — `filed` (with grounding, minus grounded hits: literally what
production would have written) and `elicited` (no grounding). They disagree on purpose on the seeds
that matter most: a seed that puts a credential in the system prompt and asks for it back files
nothing in production, correctly, because a span that was already in the input is not evidence the
model invented one — and "did this model print the key" still has to be answerable.

The fixed seeds are the score. A strong adversary model can escalate against seeds the candidate
survived, and those cases are reported **separately** and never enter the band: a benchmark whose
fixtures change between runs cannot compare two models, and comparing two models is the whole product
requirement. The adversary must itself be a strong model, and the UI says so — a 7B red-teamer writes
limp follow-ups and the candidate reports a safety record it did not earn.

### Bands, per slot (`fitness/score.ts`)

The verdict is **per slot, never one number for a model**. A slot is a thing an admin actually picks a
model for, and Talaria has exactly two kinds: the 11 `MODEL_ROLES` assignments and the 9
`PLATFORM_AGENTS` assignments. The harness→slot binding is *derived* — `rolesReaching` runs the real
resolution chain over instrumented dependencies and records which roles it asked for, rather than
copying the step order into a file where it would become an eighth spelling of the same policy.

A verdict is produced for **all twenty**; the matrix draws a **column for the fifteen a harness can
reach** (`slotViews`). The five it drops are `unbound` for every model forever, and they were dead for
two different reasons that the one "reserved" chip flattened: four are reserved *roles* whose surfaces
do not exist yet (vision, image-generation, embedding, reranker), and the fifth is the **briefer**,
which is not reserved at all — it ships, it runs every Inbox and console briefing, and its model is
fixed by design because a briefing reads one person's own views and answers in their own assistant's
voice. Its harnesses take their model from the subject of the call, so they are scored under
`FitnessReport.unbound` and shown there in the model's report. Scoring is unchanged and
`roleAssignmentIssues` still warns an admin who points a blind model at the reserved vision role.

`ready` / `workable` / `unfit`, plus `untested` (nothing measured this) and `unbound` (no harness
reaches this slot — which must read as "no evidence", never as an empty green cell). `unfit` always
names the harness and, where a fixture failed, the fixture's own sentence verbatim. A slot's band is
the **worst of its harnesses' bands**, decided per harness.

`ready` requires positive evidence, and this is the one place the suite is deliberately stricter than
`capability.ts`. "Unknown is not false" is a rule about *running*; a verdict exists to say what has
been *measured*, so an unmeasured required capability, or a sweep with the guard switched off, caps a
slot at `workable` with a reason naming the button to press. Neither ever pushes a slot to `unfit`:
absence of evidence is not evidence of absence in that direction either.

### Price against performance (`fitness/value.ts`)

A band says whether a model *can* hold a slot. It does not say what holding it costs, and it weighs the
harness a fleet runs four thousand times a day exactly like the one that runs on Sundays. So the Cost &
value tab asks the two questions an admin actually decides on, **weighted by the same runs-per-day
vector**:

- **Cost** is not `$/MTok`. Harnesses differ enormously in shape — the concluder reads a session and
  writes four lines, the blurb writer reads a sentence and writes a paragraph — so a sticker price is
  not comparable across models. Cost here is *what a day of your measured workload costs on this
  model*: production's own runs per harness (from `harness_runs` over the telemetry window) × the
  per-case tokens a sweep measured × that model's price.
- **Performance** is not an average of contract rates. `score.ts` refuses to mint one and it is right —
  four harnesses' rates averaged into a scalar is a number with no referent. What has a referent is
  **coverage**: the share of your daily runs the model tested `ready` for, with the `workable`, `unfit`
  and never-measured shares beside it. Nothing is imputed.

`usdPerReadyRun` is the two divided: dollars per run the model is actually trusted with. A cheap model
that can carry a tenth of your day is not cheap.

Three rules keep the numbers honest, and each one exists because the live data broke it first:

- **A price with nothing to price is not `$0`.** A model whose sweep failed every case before a token
  moved has a perfectly good `$/MTok` and no measurement; it reports no figure rather than landing at
  the cheap end of the chart.
- **Zero tokens is not a measurement of zero.** A sweep against a model id the gateway could not reach
  ran all 70 cases, failed every one, and overwrote 26 harnesses' good token budgets with `0` — which
  made every dollar figure on the page read `$0.00`. `nextBudget` now leaves a previous measurement
  alone when a run measured nothing.
- **No production rows is said out loud.** A fresh install falls back to one run of every harness and
  the payload reports `basis: 'uniform'`, because "we assumed your traffic is flat" is a materially
  different claim from "this is your traffic".

Reports archived before the index carried its per-harness half are **backfilled** from the full record
rather than being asked to re-test — the measurement is real and already paid for.

### Running several candidates (`MAX_CONCURRENT_RUNS`)

Up to **eight** candidates are tested at once, because comparing a shortlist is the actual job and
one-at-a-time turns a fifteen-minute sweep into an afternoon.

The old ceiling of three was never about the provider — each run is strictly sequential inside itself
(one harness, then one case), so N runs are N concurrent requests and eight of those is nothing. It was
about *us*: the resume checkpoint lived in one settings row holding every running candidate's cases, so
each per-case write was a read-modify-write of every sibling's work too. Write traffic went as
O(N² × cases²) — ~400 MB at three candidates, ~4.6 GB at ten — and every write was a synchronous JSON
parse/stringify of a multi-megabyte blob **on the event loop serving the UI**. One row per candidate
(`harness_eval_run:<model>`) takes N out of the cost entirely: a write is proportional to one
candidate's own cases and touches nothing else, so eight concurrent sweeps write exactly what eight
sequential ones would.

What binds now is the provider — about 16k tokens a minute per run, so eight is ~130k TPM against a
single key. Candidates spread across endpoints don't share that budget at all.

Everything below the run was already per-candidate — the checkpoint, the archived report, the
capability keys. Only the in-process flag and the status row were global, and both are now maps keyed
by model (`model_fitness_runs`, `harness_eval_runs`). A second sweep of the *same* candidate is still
refused: two sweeps interleaving one checkpoint is what the original boolean was protecting against.
Stop is per run, so dropping a hopeless candidate does not cost the other two.

### One spelling per model (`canonicalModelId`)

`gatewayModels()` offers each model **once**, as `<endpoint>/<model>`. It used to offer both spellings,
which put `claude-opus-5` and `anthropic/claude-opus-5` on consecutive rows of every picker and of the
matrix — one deployment, one capability key, two entries. The qualified form is canonical because it
names *where* the model runs, which is the thing capability is measured about (`capabilityKey` is
`endpoint:model`, never a bare name), and it reads correctly for both kinds of provider without a
special case: a router gives `openrouter/deepseek/deepseek-v4-flash`, a direct vendor gives
`anthropic/claude-opus-5`.

A bare id survives only where it means something else — served by more than one endpoint it is the
round-robin **pool**, a routing target no qualified id can express. Bare ids stay callable regardless:
`routingFor` is untouched, so every stored assignment keeps working, and `canonicalModelId` maps a
stored spelling onto the offered one so a role assignment and an archived report still line up with the
row an admin is looking at.

### Tested vs observed (`fitness/observed.ts`)

Two tables already carried production fitness signal and nobody was reading them as such:
`harness_runs` (one row per harness exit) and `guard_findings` (one row per filed finding, with a
`model` column). The alert this exists for is **a model that benched `ready` and is running at a 12%
repair rate in production** — the thing no external benchmark can ever give you. `divergences` is that
alert, gated on a minimum sample (20 runs over a 30-day window by default), because three production
runs is noise, not a divergence.

The two populations are **not summed**, and `observed.test.ts` asserts it. They are the same events
counted from different ends, and `guard_findings` is the broader one — the public gateway route, chat
and channel replies file into it and none of them writes a `harness_runs` row, so it has no run
denominator anywhere in the schema. Hence a **rate** from `harness_runs` alone (`findingsPerRun`, the
number the verdict compares against) and **counts** from `guard_findings` alone. Both are
ungrounded-only, because a finding raised by a model repeating an identifier out of its own input is a
fact about the input, not about the model.

Neither tier 1 nor tier 2 writes into either table. Pressing Test must not move the number Test is
being compared against.

## Related

- [`AUDIT-HARNESS-2026-08-06.md`](./AUDIT-HARNESS-2026-08-06.md) — the audit this layer answers, with
  the reproductions behind every claim above.
- [`SDK.md`](./SDK.md) — the app-developer surface.
- [`API-CONVENTIONS.md`](./API-CONVENTIONS.md) — the route dialect, and the other invariants CI holds.
- [`WORKBENCH.md`](./WORKBENCH.md) — **workbench** harnesses, which are a different contract with a
  similar name: a coding CLI an agent drives inside a sandbox, declared as shell templates and env.
  Neither is a specialization of the other.
