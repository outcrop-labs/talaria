- **Confab guard: PII check + coaching.** A fifth check, `pii_leak`, catches
  high-precision personal data in model output — SSNs, Luhn-validated payment
  card numbers, IBANs (emails/phones stay unpoliced: they're everyday
  workspace content) — and strict mode now redacts PII alongside secrets in
  whatever Talaria persists or hasn't yet relayed. And the guard finally
  closes its loop: an opt-in **"Coach agents from findings"** toggle turns
  repeated findings (≥2 of a check in 7 days) into templated behavioral notes
  in the agent's rendered soul — per-check counts + fixed advice only, so
  flagged CONTENT still never re-enters any model's context; delivery is at
  render time, a performance review between sessions rather than a mid-turn
  correction. Verified live: a raw model echoing a test SSN through the
  public route came back `[redacted SSN]` with a caveat and an out-of-band
  finding, and rendered souls gain/lose the coaching block as the toggle
  flips, 8/8 checks.
