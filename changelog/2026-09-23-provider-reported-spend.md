- **Observability spend comes from the provider.** A call stores the dollars
  the provider reported (`usage.cost`, or a later generation lookup) with the
  provider, the serving endpoint, and when it was fetched. OpenRouter activity,
  credits, and the OpenAI and Anthropic cost reports are ingested when the key
  can call them; a refusal leaves that provider unlabeled rather than guessed.
  Where a provider publishes a price and no charge, the ledger derives from
  that published rate and labels it derived. OpenRouter without the serving
  endpoint stays unpriced — its catalog price is not the charge. The Models
  page no longer edits in/out rates. Verified: `bun run check`; `cargo test --lib`
  on the touched crates only (gateway, price-oracle, fitness, harness, harness-defs,
  jobs, routes-fleet) — not a workspace build. Parser tests keep each OpenRouter
  endpoint's price and activity row separate, and a model-level OpenRouter price
  is not used when the serving variant is unknown.
