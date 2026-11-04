- **OpenRouter's router models no longer price calls at minus $1 per token.** OpenRouter's catalog lists `openrouter/auto`, `openrouter/fusion`, `nvidia/switchyard` and its other routers at `"-1"`, which is its marker for a variable price. The price check stored that as -$1,000,000 per million tokens. Since #682 lets OpenRouter rows without a serving endpoint take the model's catalog price, a call booked on a router without its own reported charge was priced at about minus its token count in dollars. Five 11k-token rows came to -$55,000, so the org's 24-hour spend went negative and every USD budget stopped binding.
  - The price check now refuses any negative or non-finite catalog number, and records the router models as **variable price**, with the reason shown in the Unpriced models panel.
  - The priced view never reads a negative rate from any source (published, manual or provider-reported) and floors every token count at zero, so no row can price below $0. The rows an instance already stored stop counting at once.
  - A migration deletes the negative rows from `provider_prices`.

  Verified:
  - `bun run gate`.
  - `cargo test -p talaria-price-oracle -p talaria-gateway --lib`: the live catalog's `-1` router entries are never priced and are recorded as variable, negative or unreadable numbers are refused, a negative cache rate drops only that rate, and the view text carries every guard.
  - `#[ignore]` live tests against a migrated Postgres:
    - five `openrouter/auto` rows on an already-stored `-1` price come out unpriced at $0 in the budget read, with the variable-price reason, while a call carrying OpenRouter's own charge is still priced;
    - a property test over 864 combinations of class, provider charge, admin decision, published rows, served variant and token counts (including negative ones written past the api) finds no cost below zero and holds the view to `price_basis` on every case.
  - On a worktree stack: an `openrouter/auto` row showed as unpriced with the variable-price reason, and the totals stayed non-negative.
