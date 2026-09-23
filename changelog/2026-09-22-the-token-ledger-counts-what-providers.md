- **The token ledger counts what providers actually bill** (#243): cache-write, cache-read
  and reasoning tokens get their own `usage_events` columns, and `normalizeUsage` detects
  each provider's shape from the payload — Anthropic native reports cache tokens OUTSIDE
  `input_tokens` (the flat model understated), OpenAI-compatible folds cached input INTO
  `prompt_tokens` at full price (it overstated). Cache writes bill at 1.25× input, reads at
  0.1×; reasoning rides inside output, recorded for visibility. Volume views and the cost
  cards total every kind now, with a standing "rate-card estimates, not invoices" footnote.
- Gateway metering no longer escapes on abort: the streaming ledger write settles exactly
  once from flush, client-cancel, or the request's abort signal — a client that hangs up
  mid-stream is still billed by the provider, so it is still recorded. Non-streaming metering
  books usage from failed responses too (a rejection without usage books nothing — a
  rejection can't invent spend).

### Added
