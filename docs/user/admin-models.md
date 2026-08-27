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
- **Pricing · $/1M tokens (in / out)** — cloud only. Fill the endpoint default as fallback,
  then per-model rows; unpriced models show as `auto` and stay unattributed in the ledger.
- **Remove provider** — cascades: every agent on it gets new versions, a re-render, and a
  restart.

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
| **Reserved** | Vision · Image-generation · Embedding · Reranker |
| Your assistant | Fixed — each person's assistant, set from their own settings |

## Fitness: test before you trust

The **Fitness** tab measures whether a model holds a slot, and what holding it costs.
Grey means nothing has measured it — **which is not a pass**. **Test a model**: pick a
candidate and the tiers to probe (optionally re-measuring capabilities already probed), and
the footer shows the bill before you start — *Start: 42 calls, $0.31*. The run charges your
provider; the long tier can be stopped at any case boundary and resumes where it left off.
The cost figure is a floor, not a total.

## Access: what members may pick

The **Access** tab limits which models non-admins may pick for AI drafting and as their
preferred model. Keep the expensive ones for deliberate, admin-configured use — agents' own
brains are set per agent and unaffected.

## API keys for other tools

Members granted **Mint API keys** can mint personal LLM-gateway keys in
**Settings → API keys** — an OpenAI-compatible base URL and model list drawn from this page —
for external tools. Keys are scoped to the models allowed here.

## Words Models uses

| Term | Meaning |
| :--- | :--- |
| **Provider** | A model backend — cloud API or self-hosted server |
| **Slot** | One job a model can hold (research-brief, judge, muse…) |
| **Tier** | A probed capability band from fitness runs |
| **`auto`** | No assignment or no price — Talaria picks, or the cost goes unattributed |
| **Gateway** | This instance's OpenAI-compatible endpoint that minted keys point at |
