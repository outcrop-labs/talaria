- **Cloud usage that a provider publishes a price for is now priced automatically, history included. An admin can fix the rest from Observability → Cost.** Two problems had left "Unpriced cloud usage" firing on every instance with no way to clear it:
  - **OpenRouter agent turns were never priced.** The priced view refused OpenRouter's own `/models` price for any call that did not name its serving endpoint. On main, every agent turn's ledger row is one of those calls.
  - **No price could be set.** The alert sent admins to `/models`, where pricing is read-only.

  Now, in order of precedence:
  - The provider's own charge comes first.
  - An admin can mark a model **included in a plan** (priced at $0, labelled `included`, never a silent zero). This beats a published price, which matters for a Z.ai Coding Plan, whose GLM ids all match OpenRouter's pay-as-you-go entries.
  - Next is the published price: the serving endpoint's when the call names one, otherwise the model's, OpenRouter included.
  - Last is an admin's **manual rate**, per 1M input, output, cache-write and cache-read tokens. It applies only until a source publishes a price.

  Other changes:
  - Cache tokens bill at the catalog's own published cache rates when it has them.
  - No row ever prices below $0, and no price that is not a finite number counts. OpenRouter publishes `"-1"` for its router models (`openrouter/auto`, `openrouter/fusion`, `nvidia/switchyard` and others), meaning the price is variable. The price check refuses that number and records those models as **variable price**, and calls that carry OpenRouter's own charge are still priced. The priced view reads no negative, NaN or infinite rate from any source and floors token counts at zero. A migration deletes any negative rows an earlier price pass stored.
  - The price check now also matches OpenAI's `-YYYY-MM-DD` snapshot ids, older `-MMDD` ids such as `gpt-4-0613`, and Fireworks-style path ids. An id whose base model the live catalog no longer lists, such as xAI's `grok-4-0709`, stays unpriced.
  - The price check records why each model has no price: no published price (and the ids it tried), the source didn't answer (quoting its error and every cause under it), or not looked up yet.
  - The new admin-only **Unpriced models** panel lists each model with its endpoint, tokens and reason, and offers **Set price**, **Included in a plan** and **Retry lookup**.
  - Pricing is read-time, so each fix re-prices that model's past calls, and the alert clears for history. The alert now goes to admins only and links to the panel.
  - The endpoint editor shows the admin's settings beside the published prices.

  New API: `GET /api/cost/unpriced`, `PUT`/`DELETE /api/cost/prices`, `POST /api/cost/unpriced/lookup`, all admin-only.

  **Upgrade note: USD budgets start binding.** On main, OpenRouter agent turns counted as $0, so an org or per-agent dollar ceiling never bound on them. After the upgrade, every row inside the budget window re-prices at once (24 hours by default, up to 365 days), and new calls carry OpenRouter's own charge. An agent whose real spend is already over its ceiling is refused with `429 budget_exceeded` on its first call after the deploy. Review the USD ceilings under **Admin → Settings** before upgrading, or right after.

  Verified:
  - `bun run gate`.
  - `cargo test -p talaria-price-oracle -p talaria-gateway -p talaria-alerts -p talaria-routes-comms --lib`, which covers id matching against ids taken from the live OpenRouter catalog, the price decision, and the reasons.
  - The `#[ignore]` live tests against a migrated Postgres:
    - history re-prices when a manual price lands, then a published price takes over;
    - included is $0 and labelled, and beats a published price but not the provider's charge;
    - OpenRouter rows without a serving variant are priced, both main's per-turn shape and rc's cut-off stream;
    - the SQL agrees with `price_basis` on nine cases;
    - a failed source's error is quoted as the reason;
    - the alert clears when a price lands.
  - The ui tests for the panel's helpers.
  - On a worktree stack, real gateway calls on a Z.ai Coding Plan endpoint, plus OpenRouter and OpenAI rows in the ledger's own shapes. With the catalog first blocked by a proxy, then reachable, each one was seen unpriced with its reason and fixed all three ways in the running app: retry, included and manual, plus the endpoint editor's plan checklist. Unpriced tokens went from 154,572 to 0, the alert cleared, members got 403s, and every write was audited.

  Also verified for the variable prices: the live catalog's `-1` entries are never priced; five `openrouter/auto` rows on a stored `-1` price come out unpriced at $0 in the budget read; a property test over every combination of sources, including negative, NaN and infinite rates and negative token counts, finds no cost below zero; and on a worktree stack an org USD budget of $0.01 refused a real gateway call once the router rows stopped counting negative.

  Not verified: a live OpenRouter or OpenAI key (none was available), and a client instance's own data.
