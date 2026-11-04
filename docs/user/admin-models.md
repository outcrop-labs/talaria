# Admin: Models

The Models page is where the org's model stack lives: providers you add, which model runs
which job, whether each model is actually good at that job, and what members may pick on
their own. Four tabs — **Models · Roles · Fitness · Access** — and the header's
**Add provider** is where everything starts.

Open: **Manage → Models**. Admins only.

## Add a provider

**Add provider** → pick from the list, then:

| Field | Notes |
| :--- | :--- |
| **Provider** | The preset (OpenAI, Anthropic, OpenRouter, Ollama, vLLM…) — sets the defaults |
| **Name** | What it's called everywhere in Talaria |
| **Base URL** | LAN and loopback hosts count as **self-hosted** in the cost split |
| **API key** | Stored encrypted at rest (AES-256-GCM) — never written to a config file, never shown again |
| **Advanced: env-var fallback** | Optional env-var name to read the key from, if none is stored |

Each provider card shows its class — `self-hosted` or `cloud` — model count, and base URL.
Self-hosted hardware and cloud keys sit in separate sections.

## Manage a provider

**Manage** on the card. The key section says `● encrypted key stored` or
`none stored, using $ENV`; paste a new key to rotate. Then:

- **Available models** — trim the list to what you actually carry; add from the provider's
  live catalog.
- **Reasoning effort** — declare the levels each model accepts; they're sent verbatim.
- **Pricing** — cloud only. Published rates come from the provider, with the source and when
  they were fetched, and are read-only. Spend uses the provider's reported charge when they send
  one, then the published price: OpenRouter's per-endpoint price when the call names the endpoint
  that served it, otherwise the model's published price. A model no source prices shows up under
  **Observability → Cost → Unpriced models**, where an admin sets a rate or marks it included in a
  plan; those settings are listed here too.

### When usage has no price

**Observability → Cost** lists, for admins, every cloud model whose last 30 days of usage has no
price. Each row shows the endpoint, the model, the tokens, and why:

| Reason | What it means |
| :--- | :--- |
| **no published price** | The price check asked OpenRouter's public catalog and found no entry for the id, or for the shapes it also tried: the id with a snapshot date, a `:tag` or `-latest` removed, and the last segment of a path id such as `accounts/fireworks/models/<id>`. The row also says when a shape was found but withheld: a snapshot whose listed siblings are priced differently from its alias, or an OpenRouter `:online` / `:nitro` id with no entry of its own |
| **variable price** | The catalog lists the model with a variable price (OpenRouter's routers, such as `openrouter/auto`), so there is no per-token rate. Calls that carry OpenRouter's own charge are still priced |
| **suggested price** | Another host's model matched an OpenRouter entry only by its last path segment. OpenRouter's rate is offered (**Use … rate**), never applied, because that host may bill differently |
| **source didn't answer** | The catalog could not be read. The row quotes the error it gave |
| **not looked up yet** | The check runs every 6 hours and soon after new usage |
| **endpoint removed** / **no endpoint recorded** | The usage was booked under an endpoint that no longer exists, or under none, so no endpoint's published price applies |

Each row offers three fixes:

- **Set price** — dollars per 1M input and output tokens, plus cache write and cache read if you
  know them. Without cache rates, cache tokens bill at the input rate × 1.25 and × 0.1.
- **Included in a plan** — for a flat-rate plan, such as a coding plan or a subscription, or a free
  tier. Calls are priced at $0 and labelled *included*, never shown as a silent zero. This applies
  even where a published pay-as-you-go price exists, because a catalog cannot know your account is
  on a plan.
- **Retry lookup** — run the price check for that model now.

A fix re-prices that model's past calls straight away, so the spend figures and the **Unpriced
cloud usage** alert update for history as well as new usage. The order is fixed:

1. The provider's own charge.
2. *Included in a plan*.
3. A published price found by the model's own id.
4. Your manual rate. It applies only until a published price for the model's own id appears, and
   the panel says when one has taken over. **Remove** undoes a setting.
5. A published price found by a fuzzy match (a stripped date or tag, or a path's last segment).
   **Priced by a fuzzy match** lists these with the entry each was priced as, and the cost page's
   legend says "priced as … (fuzzy match)". Set a manual rate to override one.

A manual rate of $0 in and $0 out is refused: say *Included in a plan* instead, so a free model
never reads as a silent zero. The 30-day spend tile also says how much of its total is estimated
from published prices rather than charged by the provider.

**Dollar budgets count all of this.** Budgets read the same prices, so a price that appears, from
the catalog or from you, also counts toward every USD ceiling whose window holds those calls.
After upgrading from a release that left OpenRouter agent turns at $0, that history re-prices at
once, and an agent already over its ceiling is refused on its next call. Check the ceilings under
**Admin → Security → LLM budgets** when you upgrade.
- **Remove provider** — cascades: every agent on it gets new versions and a re-render, and each
  running one restarts onto them (offline for about a minute). An agent someone stopped stays
  stopped and picks the change up when it next starts.

## Roles: which model runs what

The **Roles** tab assigns a model to every job in the org. Pick a slot, pick a model; empty
slots run `auto`. Unfit assignments surface as warnings — they report, never block.

| Area | Slots |
| :--- | :--- |
| **Research** | Recon · Brief · Expedition |
| **Workbench** | Code-light · Code-standard · Code-heavy |
| **Chores** | Utility — plus the platform workers: blurb-writer, titler, summarizer, librarian |
| **Writing** | Muse · Distiller · Concluder · Briefer |
| **Oversight** | The QA judge |
| **Agents' eyes** | Vision — reads images for every agent whose own model is not known to see; leave it empty and those agents cannot read an image |
| **Reserved** | Image-generation · Embedding · Reranker |
| Your assistant | Fixed — each person's assistant, set from their own settings |

**How an agent sees.** Talaria knows a model can read images when a fitness run's vision probe
passed, the provider's catalog lists image input, or you declared it. An agent whose main model
is known to see gets images as pixels. Any other agent gets them through the Vision model, which
describes each attached image and also reads the images tools hand back. Saving the Vision slot
re-renders every agent. A new capability fact reaches agents the next time the fleet renders (any
agent edit, **Roll**, or deploy). An agent has one image mode: if its main model sees but one of its
tiers or fallbacks does not, a turn on that tier or fallback cannot read an attached image.

## Fitness: test before you trust

The **Fitness** tab measures whether a model holds a slot, and what holding it costs.
Grey means nothing has measured it — **which is not a pass**. **Test a model**: pick a
candidate and the tiers to probe — the capability bands fitness sorts models into, a
different sense of "tier" than an agent's model aliases — and the footer shows the bill
before you start: *Start: 42 calls, $0.31*. The run charges your provider; the long tier
can be stopped at any case boundary and resumes where it left off. The cost figure is a
floor, not a total.

## Access: what members may pick

The **Access** tab limits which models non-admins may pick for AI drafting and as their
preferred model. Keep the expensive ones for deliberate, admin-configured use — agents' own
brains are set per agent and unaffected.

## API keys for other tools

Members granted **Mint API keys** can mint personal LLM-gateway keys in
**Settings → API keys** — an OpenAI-compatible base URL and model list drawn from this page —
for external tools. Keys are scoped to the models allowed here.

Each key carries its own ceilings (**Limits…** on the row): a spend cap in tokens and/or
dollars over the org budget window, and a requests-per-minute throttle. They are the owner's
to set — a leaked or runaway key then burns only what its owner decided it may. A personal cap
can only tighten, never raise past, an admin's ceiling; hitting it answers `429 budget_exceeded`
(or `rate_limit_exceeded`) with `retry-after`, which standard OpenAI clients honor. A budget
refusal also says when the window's spend falls back under the cap, if nothing more is spent:
`error.resets_at` (epoch seconds) and the same time in its message.
