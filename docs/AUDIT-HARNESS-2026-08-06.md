# Talaria — harness integrity audit + the harness framework

_2026-08-06 · against `9b2b134` · worktree `wt/harness-audit` · ~28k LOC in `ui/src/server`, 384 `.svelte`, 166 API routes_

Scope: everywhere Talaria puts words into a model and reads something structured back. Three
questions, in order:

1. **Does the harness layer hold today?** (application-level integrity)
2. **Can we tell whether a given model is a good fit, per role, from inside the UI?** (the self-test)
3. **Is there one way to build a harness, and is it in the SDK?** (the framework)

Findings marked **[verified]** were reproduced by execution, not read for.

---

## Verdict

The *authorization* layer of this codebase is genuinely excellent, and it got that way by a specific
method: a predicate gets centralized, then `scripts/check-invariants.mjs` grows a rule that fails the
build on the ninth hand-written copy. `agentTicketRefusal`, `statusMeta.activeKey`, `audienceFor`,
`logTicket` — one door each, with a tripwire under it. That method works and it is the thing to copy.

**The harness layer never got that treatment.** It is at the stage the authorization layer was at
around round three: the *concept* is centralized (`PLATFORM_AGENTS` is a real registry, `MODEL_ROLES`
is a real contract, `defineHarness` is a real extension point) but the *behavior* is nine hand-written
copies. There are:

- **6 different structured-output extractors**, three of which fail on realistic small-model output **[verified]**
- **6 verbatim copies** of the model-fallback chain
- **2 different structured-output request strategies** (`response_format` vs. a prompt suffix) on the
  two halves of one feature
- **3 high-stakes model paths with no guardrail at all** — agent work sessions, research synthesis,
  Muse drafting
- **0 evals**, **0 model-fitness signal**, and **0 retry** on a malformed structured response anywhere

None of this hurts much on Opus or Sonnet, which is why it shipped. All of it hurts on a 14B local
model, which is the stated product requirement. The failure mode is not an error — it is a `return
null` that a fire-and-forget caller swallows, so a bad model presents as a feature that quietly stops
working.

The good news is that the fix is one refactor, not five: **the registry that already exists should
carry the prompt, the schema, the chain, and the eval fixtures.** Once it does, the model self-test is
not a new subsystem — it is `for (const harness of REGISTRY) run(harness.evals, candidateModel)`.

---

# Part 1 — Integrity findings

## P0 — Broken for small models now

### 1.1 Three of six JSON extractors fail on ordinary small-model output [verified]

Six distinct extraction strategies exist. No shared helper.

| Where | Strategy |
|---|---|
| `server/judge.ts:107` | greedy `/\{[\s\S]*\}/` → `JSON.parse` → escalate on failure |
| `server/model-info.ts:139` | greedy `/\{[\s\S]*\}/` → `JSON.parse` → `return 0` (silent) |
| `server/inbox-focus-assistant.ts:5` | `indexOf('{')` / `lastIndexOf('}')` |
| `lib/muse.svelte.ts:87,111,158` | greedy `/\{[\s\S]*\}/`, client-side, ×3 |
| `server/research.ts:186` | non-greedy `/\[[\s\S]*?\]/` **+ a line-based fallback** |
| `server/titler.ts:58`, `skill-summaries.ts:68` | first non-empty line, strip quotes/fences |
| `server/kb-okf.ts:54` | `/^TAGS:\s*(.+)$/m` |

Both brace strategies are "first `{` to last `}`", which is not a JSON scanner. Reproduced against
three shapes a 14B model emits constantly:

```
FAIL  greedy regex (judge/muse/model-info)    fenced + trailing prose w/ brace
FAIL  indexOf/lastIndexOf (inbox-focus)       fenced + trailing prose w/ brace
FAIL  greedy regex (judge/muse/model-info)    preamble + two objects
FAIL  indexOf/lastIndexOf (inbox-focus)       preamble + two objects
FAIL  greedy regex (judge/muse/model-info)    object then bulleted prose
FAIL  indexOf/lastIndexOf (inbox-focus)       object then bulleted prose
OK    greedy regex (judge/muse/model-info)    clean object (control)
OK    indexOf/lastIndexOf (inbox-focus)       clean object (control)
```

A brace-balancing scanner that respects string literals handles all three, and `research.ts`'s
non-greedy-plus-fallback already half-knows this. The lesson `research.ts` learned never propagated,
because there was nowhere to put it.

**Consequence per site**, which is the real problem — each failure is a *different* silent behavior:

- `judge.ts` → verdict `escalate`, "returned no parseable verdict". A weak judge model therefore
  escalates constantly, and with `mode: 'enforcing'` (the default in `DEFAULT_CONFIG`) every
  escalation notifies board editors. A bad judge model is a notification storm, not an error.
- `model-info.ts` → `return 0`. Blurbs never get written, `maybeRewriteBlurbs` re-burns the batch
  every 10 minutes forever, nothing is logged.
- `muse.svelte.ts` → `null`. The "design an agent" / "draft a cron" flows just do nothing on click.
- `inbox-focus` → `null` → falls through to "I could not safely map that instruction."

**Fix:** one `parseJson(text, schema)` in a shared module — balanced-brace scan, fence stripping,
trailing-comma tolerance, then Zod validation. One behavior on failure, declared by the harness.

### 1.2 The gateway can silently strip `response_format`, and the strip is permanent

`llm-gateway.ts:141-225` learns unsupported parameters from upstream 400s and pre-strips them on
later calls. This is a genuinely good idea — model specs rot, providers don't. Three problems:

- `rejectedParam` matches `[a-z_]+`, so **`response_format` is strippable**. A model that rejects
  `{"type":"json_object"}` gets the parameter removed and the call *succeeds*, returning free prose.
  The caller has no idea its structured-output constraint was dropped and hands prose to a JSON
  extractor. The only mode where the harness knew it was in structured mode silently becomes the mode
  where it isn't.
- Learnings persist to `app_settings` (`gateway_unsupported_params`) with **no TTL and no
  invalidation** (`persistUnsupportedParams`, line 151). A provider fixing support, or a model id
  being reused, never clears it. This is a one-way ratchet on capability.
- `buildUpstream` strips silently — no log, no return signal, nothing on `/observability`.

**Fix:** classify learned params. Cosmetic ones (`temperature`, `top_p`) strip silently as today.
**Contract-bearing ones (`response_format`, `tools`, `tool_choice`) must never be silently stripped** —
they should surface as a typed capability fact ("this endpoint:model cannot do JSON mode"), fail the
structured call over to a text-plus-repair path deliberately, and be recorded as a *model capability*,
which is exactly what the self-test in Part 3 wants to read. Add a TTL (30 days) plus an admin "forget
what you learned about this model" button.

### 1.3 Two structured-output strategies inside one feature

`inbox-focus-assistant.ts` has both:

```ts
requestJsonObject()         // proxyChat, response_format: { type: 'json_object' }, temp 0.1
requestGatewayJsonObject()  // completeViaGateway, NO response_format,
                            // prompt suffix "Return exactly one JSON object…", temp 0.2
```

`runFocusCommand` picks between them by whether the user chose a `responseModel`
(`inbox-focus.ts:893-895`). So **the same command, on the same item, is a strict-JSON request on one
model path and a prompt-and-pray request on the other** — different temperature, different reliability,
no note anywhere that they differ. `completeViaGateway`'s signature simply has no slot for
`response_format`, so the second path could not have done better without changing the shared helper.

### 1.4 Nothing retries a malformed structured response. Anywhere.

Zero call sites re-ask. The single cheapest, highest-yield small-model accommodation in the industry —
"that wasn't valid JSON, here is the parser error, return only the object" — is not implemented once.
`fetchUpstreamInner` has a 4-attempt loop, but it is for *parameter rejection*, not content validity.

For a frontier model this is worth ~0. For a 14B model, a single repair round-trip typically converts
the large majority of structured-output failures. This is the highest-leverage item in the document.

## P1 — Coverage gaps

### 1.5 Guardrails are attached per-call-site, so the three highest-stakes paths bypass them

`guardCompletion` / `guardChatReply` / `guardText` are wired into: the public gateway route
(`llm.v1.chat.completions.ts`, 4 sites), `completeViaGateway`, chat persistence, channel replies, and
the judge's pre-check. Not wired into anything that calls `proxyChat` or `fetchUpstream` directly:

| Unguarded path | What flows through it |
|---|---|
| `work-dispatch.ts:151` `sendTurn` → `proxyChat` | **Agent work-session turns** — the agent reporting a ticket is done. `zero_tool_claim` exists *precisely* for this and does not run here. |
| `research.ts:159` `personaStage` → `proxyChat` | Research synthesis. `ungrounded_ref` — invented citations — is the definitive research failure mode and is not checked. |
| `research.ts:118` `searchStage` → `fetchUpstream` | Search-stage output and its source list. |
| `routes/api/muse.ts:44` → `fetchUpstream` (streamed) | Drafted SOUL.md / skills / memories. A drafted soul containing a credential is neither flagged nor redacted. |
| `inbox-focus-assistant.ts:53` `requestText` → `proxyChat` | Personal-assistant replies — **but only on the persona path**; the sibling `requestGatewayText` goes through `completeViaGateway` and *is* guarded. The same reply is guarded or not depending on which model the user picked. |

This is not a guardrail design problem. `guardrails.ts` is the best-designed file in the harness layer:
a real rule registry, `needs`/`Available` capability negotiation so a path that can't supply tool
results *skips* a rule rather than false-positiving, and a hard, documented invariant that flagged
content never re-enters a model's context. It is a *placement* problem — the guard lives in two of the
five ways to reach a model, and there is no chokepoint that forces the other three through it.

### 1.6 Model roles are validated for routability, never for capability

`resolveRoleModel` (`model-roles.ts:116`) returns the assignment "only while it still ROUTES." That is
the whole check. Consequences:

- An admin can assign any routable model to `research-recon`. `searchModelFor` returns it, and the
  research pipeline runs its search stages on a model with **no web search**, producing a confident,
  uncited, hallucinated brief. The auto-fallback carefully prefers `sonar*`; an explicit assignment
  bypasses that reasoning entirely.
- `code-light|standard|heavy` accept any routable model, including one with no tool-calling.
- `vision`, `image-generation`, `embedding`, `reranker` are `wired: false` — declared but inert.

There is no notion of a **model capability** in the codebase. That is the gap Part 3 fills.

### 1.7 `pl-main` is hardcoded as a fallback in 7 places

`titler.ts:31`, `skill-summaries.ts:40`, `model-info.ts:95`, `kb-okf.ts:21`, `muse.ts:127`,
`workbench-harnesses.ts:161`, `judge.ts:214`. On an install that never named an endpoint `pl-main`
these resolve to nothing and the subsystem silently no-ops. It is a reasonable default for the
reference deployment and a poor one for a self-host, and it is spelled out rather than derived.

### 1.8 The Inbox command harness gives a stronger model nothing to do

`runFocusCommand` sets `allowedActionIds = deterministic ? [deterministic.actionId] : []`
(`inbox-focus.ts:857`). `deterministicProposal` is three regexes. So:

- If a regex matched, the model may only confirm the single action the regex already chose.
- If no regex matched, `allowedActionIds` is empty and the model may only clarify.

The model can **never** select an action. This is the safest possible design and I would not weaken it
casually — `validateCommandObject` rejecting an out-of-list `actionId` is exactly right. But it means
the product's headline "assistant that acts on your inbox" is, in action-selection terms, a regex, and
upgrading to a frontier model buys nothing here. Worth an explicit decision rather than an emergent
one: a capability-gated widening (models that pass the tool-selection eval in Part 3 get the full
action list; others stay regex-bound) is the natural shape, and it is exactly what "excel with larger
ones" means.

### 1.9 `check-invariants.mjs` is red on `main` and blind to 384 files [verified]

```
FAIL  off-board-status-literal          ui/src/components/board/field-pills.ts:80
FAIL  off-board-status-literal-census-stale   (4 entries name .tsx files deleted in the Svelte migration)
2 invariant check(s) failed.
```

Also `EXTS = ['.ts', '.tsx', '.mts', '.js', '.mjs']` (line 65) — **no `.svelte`**. 384 `.svelte` files,
now the bulk of the UI, are outside every rule. The repo's best mechanism stopped covering the repo.

Unit tests pass: **11 files, 194 tests, 2.58s** [verified]. `routes/api/mcp.test.ts` is correctly
excluded (it is a route, not a test). 194 tests over 28k server LOC with zero covering the harness
layer.

## P2 — Duplication that will re-break

### 1.10 The fallback chain, six times, verbatim

```ts
const pinned = await platformAgentModel('<id>')
if (pinned) return pinned
const utility = await resolveRoleModel('utility')
if (utility) return utility
for (const m of [process.env.TALARIA_COPILOT_MODEL ?? null, 'pl-main']) {
  if (m && (await resolveRoute(m))) return m
}
return (await gatewayModels()).find((m) => !m.qualified)?.id ?? null
```

Identical in `titler.ts:26`, `skill-summaries.ts:35`, `model-info.ts:89`, `kb-okf.ts:16`. Variants in
`muse.ts:112` (adds allowlist + preference), `workbench-harnesses.ts:149` (adds effort fall-down),
`comms-decay.ts:58,209` (`platformAgentModel ?? museModelFor`). Seven spellings of one policy — the
exact shape `check-invariants.mjs` exists to catch, in a domain it has no rule for.

### 1.11 `PLATFORM_AGENTS` is metadata with no contract

`platform-agents.ts` declares 9 harnesses with `id / label / job / skills / auto / assignable`. It is
the right idea and it is the seed of the whole framework. But it carries none of the things that make
a harness a harness: no prompt, no output schema, no failure policy, no capability requirement, no
model chain, no evals. Those live in 8 other files, hand-written. `PLATFORM_AGENTS[].skills` is
prose in a chip; `PLATFORM_AGENTS[].auto` is a *description* of a chain implemented elsewhere and free
to drift from it.

(One thing that is right and worth recording: the Judge's model lives in `judge_config` rather than
`platform_agent_models`, and `admin.platform-agents.ts` explicitly reads and writes it there so there
is one source of truth. That is the correct handling of the exception.)

### 1.12 `docs/SDK.md` documents a React SDK that no longer exists

`.tsx`, `app.tsx`, `react`, `react-dom`, `lucide-react`, `@tanstack/react-query`,
`@tanstack/react-router`. `vitest.config.ts` still explains itself in terms of "TanStack Start."
Cosmetic, but the SDK doc is the contract for third-party app authors and it is wrong.

---

# Part 2 — The harness framework

## The convention

There are five ways to reach a model today (`proxyChat`, `completeViaGateway`, `buildUpstream` +
`fetchUpstream`, the `/api/llm` route, `requestJson*`), and each harness picks one, writes its own
prompt, its own parser, its own chain, and its own failure behavior. That is the "20 different ways"
problem stated precisely.

**One way:** a harness is a declarative object. It never chooses a transport, a model, a parser, or a
failure policy — it *declares* them, and one runner honors the declaration.

```ts
// server/harness/define.ts — the whole contract
export interface HarnessDefinition<I, O> {
  id: string
  label: string
  job: string                      // one line, shown in Admin (today's PLATFORM_AGENTS.job)

  /** What the model must be able to do. The self-test scores against this. */
  requires: Capability[]           // 'json' | 'tools' | 'search' | 'vision' | 'long-context' | 'code'

  /** Model resolution, declared not written. Replaces the 6 copies of the chain. */
  model: {
    pin?: PlatformAgentId          // admin assignment slot (Models → Platform)
    role?: ModelRole               // then the model role
    chain?: ModelChainStep[]       // then a declared chain; default: ['utility', 'env', 'first-routable']
  }

  /** Input → messages. The ONLY thing a harness author writes by hand. */
  render: (input: I) => Promise<Message[]> | Message[]

  /** Output contract. Presence of `schema` puts the runner in structured mode. */
  output:
    | { kind: 'text'; clean?: (s: string) => string | null }
    | { kind: 'json'; schema: z.ZodType<O>; repair?: number /* default 1 */ }

  /** What happens when the model fails the contract after repair. Explicit,
   *  because today each of the 6 sites answers this differently and silently. */
  onFailure: 'null' | 'throw' | { fallback: O } | { escalate: true }

  /** Guardrails: which rules apply and what the runner may supply them. */
  guard?: { rules?: string[]; redact?: boolean }

  temperature?: number
  /** Fixtures the model self-test replays. See Part 3. */
  evals?: EvalCase<I, O>[]
}
```

And one runner:

```ts
export async function runHarness<I, O>(def: HarnessDefinition<I, O>, input: I, ctx: RunContext)
  : Promise<HarnessResult<O>>
```

`runHarness` is the single chokepoint, and it does, in order:

1. **Resolve the model** from `def.model` — the chain, expressed once. Records *which* step won.
2. **Check capability.** If `requires` includes `json` and the resolved model's capability record says
   it can't, take the text-plus-repair path deliberately instead of discovering it from a 400.
3. **Render + call.** Chooses transport by whether the model is a persona (`proxyChat`) or a gateway
   model (`completeViaGateway`) — a decision no harness author should ever make again.
4. **Parse.** One balanced-brace extractor, then `schema.parse`. On failure, **repair**: re-ask once
   with the parser error appended. This is finding 1.4, fixed for every harness at once.
5. **Guard.** Runs `runGuardrails` with an honest `Available` for the transport actually used. This is
   finding 1.5, fixed for every harness at once, including the three paths that bypass it today.
6. **Meter + record.** `recordGatewayUsage` / `recordUsage`, plus a `harness_runs` row: harness id,
   model, chain step, repair count, schema-valid, latency, findings. **This row is the self-test's
   production ground truth** — see Part 3.

Everything in Part 1's P0/P1 is a property of `runHarness`, not of nine call sites. That is the point.

## Migration, in dependency order

Each step is independently shippable and each one deletes code.

| Step | Work | Deletes |
|---|---|---|
| 1 | `server/harness/json.ts` — balanced-brace extractor + `parseJson(text, schema)`, with the three failing fixtures from 1.1 as unit tests | 6 extractors |
| 2 | `server/harness/model.ts` — `resolveHarnessModel(def.model)` | 6 chain copies |
| 3 | `runHarness` + `harness_runs` table | — |
| 4 | Port the 6 leaf harnesses (titler, summarizer, librarian, blurb-writer, distiller, concluder). Smallest, most mechanical, lowest risk. | ~200 LOC |
| 5 | Port judge, muse, inbox-focus-command. These have real output contracts and gain the most. | ~150 LOC |
| 6 | Bring the unguarded paths in (work-dispatch turns, research stages, muse stream) | closes 1.5 |
| 7 | Capability classification in the gateway's param learner (1.2) + TTL | — |
| 8 | `check-invariants.mjs`: add `.svelte` to `EXTS`, fix the two red rules, add a **harness rule** — "a call to `proxyChat`/`completeViaGateway`/`fetchUpstream` outside `server/harness/` and `llm-gateway.ts` is a hand-written harness; declare it instead", with a census for the legitimate exceptions | prevents round nine |

Step 8 is the one that makes the rest permanent. Without it this document schedules its own sequel.

## The SDK surface

`sdk/server.ts` already exports `defineHarness` for **workbench** harnesses (coding CLIs in a sandbox).
That name is now overloaded, and the overlap is real: both are "a declarative description of something
that does work, merged builtin < app-shipped < admin-custom." Suggested split, keeping the existing
export working:

```ts
// @talaria/sdk/server
export const defineWorkbenchHarness = defineHarness   // today's; keep the old name as an alias
export const defineHarness = <I, O>(h: HarnessDefinition<I, O>) => h  // the new, general one
export type { Capability, EvalCase, HarnessResult }
```

An app ships `apps/<slug>/harnesses/*.ts` and the host discovers them with the **same three-layer merge
`listHarnessDefs` already implements** (`workbench-harnesses.ts:107`) — builtin < app-shipped <
admin-custom, by id. That function is the proof the pattern works; it just needs a sibling for
activity harnesses. App-shipped harnesses carry code (`render`, `clean`), admin-registered JSON ones
are declarative-only, exactly as the workbench registry already distinguishes.

What an app author gets: declare a harness, and it automatically has an admin model assignment slot, a
guardrail pass, ledger attribution, repair-on-malformed, and **a row in the model self-test** — without
writing a single line of any of it.

---

# Part 3 — The model fitness suite

## What it has to answer

Not "is this a good model." Specifically: **"for each role and each harness in *my* Talaria, is this
model good enough, and where exactly does it break?"** So that a new release is a fifteen-minute
Admin → Models → Test run and a swap, not a week of production surprises.

## The insight that makes it cheap

Once Part 2 lands, **every harness already carries its own eval.** `def.evals` is fixtures for the
contract the harness declares, `def.requires` is the capability list, and `runHarness` is the executor.
The suite is not a new subsystem — it is a driver over the registry with the model pinned.

Three tiers, cheapest first, so a bad model is rejected in seconds rather than dollars.

### Tier 1 — Probes (~10 calls, seconds, cents)

Model-level facts, harness-independent. Answers `requires`.

| Probe | Method | Feeds |
|---|---|---|
| `json` | ask for a trivial object with `response_format`; then again without | `requires: 'json'`; **and detects the 1.2 silent-strip case directly** |
| `json-strict` | 5 objects with nested arrays + a long string field | schema-conformance rate |
| `tools` | one tool definition, one call that requires it | `requires: 'tools'` |
| `tool-select` | 4 tools, 4 prompts, one correct each | tool-selection accuracy — gates 1.8 |
| `instruction-floor` | "reply with exactly the word OK" | the classic small-model tell |
| `search` | a question answerable only with fresh data + citation shape | `requires: 'search'`; **closes 1.6** |
| `long-context` | needle at 50%/90% of the model's advertised window | `requires: 'long-context'` |
| `refusal-floor` | a benign request commonly over-refused | usability |
| `latency/cost` | p50/p95 TTFB from the existing `gatewayPulse` ring | the "cheap enough for utility" call |

Output: a **capability record** per `endpoint:model`. This is the missing type from 1.6, and it should
be what `resolveRoleModel` consults before honoring an assignment — so assigning a non-search model to
`research-recon` warns at assignment time instead of producing an uncited brief in production.

### Tier 2 — Harness conformance (one pass per harness, ~1-3 calls each)

Replay `def.evals` through `runHarness` with the candidate pinned. Per harness, scored:

- **contract rate** — schema-valid on the first attempt
- **repair rate** — valid after one repair (the 1.4 metric; a model at 40/95 is *usable*, one at 40/45 is not)
- **guard rate** — findings per run from the guardrail pass that is now inline
- **task score** — harness-specific, and mostly deterministic:

| Harness | Assertion — no judge model needed |
|---|---|
| titler | 3-7 words, no quotes, no "Chat about", not a restatement |
| summarizer | ≤140 chars, one sentence, no "This skill" |
| librarian | has `## Key facts`, has `TAGS:` line, ≤5 lowercase tags |
| muse:cron | parses; `schedule` is a valid cron or interval; `name` is kebab |
| muse:agent | `handle` survives the `ident()` coercion at ≥2 chars; `soul` has all three required headings |
| muse:ticket | only fields the instruction asked for; no invented status |
| inbox:command | never proposes an `actionId` outside the allowlist ← **the safety assertion** |
| judge | labeled fixture set: 5 good outcomes, 5 with a planted gap, 2 ambiguous. Score = agreement. |
| research:queries | returns a JSON array of N distinct, non-restating queries |
| distiller | contains the 3 planted decisions, contains none of 2 planted distractors |

Most of these are string assertions, not LLM-as-judge. That keeps the suite fast, deterministic, and
free of the "who judges the judge" regress. Only `judge` needs labeled fixtures, and labeled fixtures
are cheaper and more honest than a meta-judge.

### Tier 3 — Adversarial / safety (opt-in, the expensive one)

Replays the `guardrails.test.ts` corpus and the confab fixtures *as generation prompts*: does this
model, in this harness, produce zero-tool claims, ungrounded refs, fabricated outages, secret echoes?
Scored with the existing `RULES` registry — no new detection logic, and the numbers are directly
comparable to production because production uses the same rules.

This is also the one place a strong model is required (the adversary), and the suite should say so.

## Scoring and the verdict

Per model, per **role**, three bands rather than a number:

- 🟢 **Ready** — passes every `requires` for every harness bound to the role; contract ≥95%; task ≥ the
  role's floor; guard findings ≤ the current production baseline for that role.
- 🟡 **Workable** — contract 80-95% *or* repair-rate ≥95%; task within 10% of floor. Usable with the
  repair path; surface the specific weakness.
- 🔴 **Not a fit** — a missing capability, contract <80%, or a safety regression. Name the harness and
  the assertion that failed, not a score.

The verdict is per role, not global, because that is the actual decision: a model can be 🟢 Utility and
🔴 Judge, and today nothing tells you that until the judge starts escalating everything.

## Production telemetry closes the loop

Two data sources already exist and nobody is reading them as model-fitness signal:

- **`guard_findings.model`** — `guardStats()` already aggregates by check. Aggregate by model and you
  have a live confabulation rate per model. `guardCoachingFor(model)` already does exactly this query
  for a different purpose.
- **`harness_runs`** (new, from Part 2 step 3) — contract rate, repair rate, and chain-step-that-won,
  per harness per model, in production.

So the fitness page shows **tested** and **observed** side by side. A model that benched 🟢 and is
running at a 12% repair rate in production is the alert that matters, and it is the thing no external
benchmark can ever give you.

## Surface

`Admin → Models → Fitness`, reusing the existing panel grammar.

- **Matrix.** Rows = registered models, columns = the 11 roles. Cells 🟢/🟡/🔴/untested. This is the
  "can I swap this in" answer at a glance.
- **Test a model.** Pick model → pick tiers → estimated cost and call count **before** you start (the
  price oracle already has the numbers) → live progress → drill into any failing assertion with the
  actual prompt and the actual response. The drill-down is what makes it trustworthy.
- **Compare.** Two models side by side, per harness, with the deltas.
- **Assignment guardrail.** When an admin assigns a model to a role it tested 🔴 for, say so inline
  with the failing assertion, and let them do it anyway. This is finding 1.6's fix, delivered as a
  sentence in the UI rather than a validation error.

## SDK exposure

```ts
import { defineHarness, defineEvals } from '@talaria/sdk/server'
```

An app-shipped harness that declares `evals` gets its own column in the org's fitness matrix
automatically. That is the payoff of putting evals in the registry rather than in a test directory:
**a third-party app can tell an admin which of their models it works on**, and it costs the app author
a fixture array.

---

## Recommended order

1. **1.4 + 1.1** — the JSON extractor and one repair round-trip. Two days, and it is most of the
   small-model story on its own.
2. **1.2** — stop silently stripping `response_format`; add the TTL.
3. **1.9** — get `check-invariants.mjs` green and teach it `.svelte`, before adding a harness rule to it.
4. **Part 2 steps 1-5** — the runner and the ports. Deletes ~350 LOC.
5. **1.5** — bring the three unguarded paths through the runner.
6. **Part 3 Tier 1 + the matrix** — probes and capability records are ~20% of the suite's work and
   answer the "can I swap this model in" question on their own.
7. **Part 3 Tiers 2-3, SDK exposure.**
8. **1.6, 1.8** — capability-gated role assignment and capability-gated action widening. Both are
   trivial *once capability records exist*, and impossible before.

`docs/SDK.md` (1.12) can be fixed at any point and should be, since it is the contract app authors read.
