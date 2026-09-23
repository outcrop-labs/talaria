- **A provider's exhausted account limit reads as itself, not as a generic
  upstream 400.** The upstream boundary replaced every provider message with
  `upstream error (<status>)`, so a deployment whose model had hit its monthly
  ceiling showed that sentence on every chat turn: the reader went looking for
  a malformed request while the actual answer was billing or a reset date. The
  boundary now classifies structure it already reads — the OpenAI-compatible
  quota codes (`insufficient_quota`, `quota_exceeded`, `usage_limit_reached`,
  `billing_hard_limit_reached`, `credit_balance_too_low`) and Anthropic's
  monthly-limit sentence, whose reset instant rides in our own words
  (`the upstream account's usage limit is exhausted — access returns
  2026-10-01 at 00:00 UTC`). The provider's prose still does not cross: a tail
  that is not an instant to the token — prose, or a date with anything
  appended — falls back to the generic sentence.
  Verified: `cargo test -p talaria-error` pins the incident's verbatim
  Anthropic body and the OpenAI-compatible code spelling (both produce the
  account sentence) and two look-alikes — a prose tail, and a real instant with
  a hostname and a key appended — which stay generic and leak none of it.
