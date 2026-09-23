- **Google sign-in speaks Rust too — and with it the session batch is whole**:
  `GET /api/auth/google` and its `/callback` are serving from the Rust api, the
  OAuth dance hand-rolled over reqwest (no SDK — the identity comes from
  Google's userinfo endpoint, never a locally-verified JWT). The consent URL
  byte-matches TS's own `URLSearchParams` serialization (pinned by test, then
  diffed live against the TS serializer with the same client record and state
  token), the state cookie is the same double-submit CSRF token with its
  10-minute TTL and constant-time compare, and every failure door bounces to
  `/login` with the machine-readable reason the SPA already renders. Verified
  against the live dev stack: the disabled gates byte-diffed equal on both
  routes; the client-precedence rule (a complete Admin record beats env, and
  the env pin is what enables login — the record alone never does) exercised
  for real; bad-state and cookie-mismatch bounces equal; and a live token
  exchange against Google with a garbage code returns the fixed
  `exchange_failed` sentence with the provider's prose dead in the log — the
  boundary rule, held. The happy path past Google's consent screen (claim,
  org-domain gate, invites) needs a human signing in, so those doors are
  verified to their last reachable hop, not past it. The coexistence proxy now
  forwards the caller's origin (`x-forwarded-proto`/`host`) so the two runtimes
  derive the same `redirect_uri` through it.
