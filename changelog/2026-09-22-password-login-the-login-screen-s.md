- **Password login, the login screen's door list, and the first-run claim speak Rust too**:
  the session batch's second slice — `POST /api/auth/password`, `GET /api/auth/providers`,
  `POST /api/auth/claim` — is serving from the Rust api. The credential check is the same
  scrypt contract both runtimes share (`scrypt$N$r$p$salt$hash` entries, parameters read
  from the entry, the dummy-hash burn that keeps an unknown email exactly as slow as a
  wrong password), run on the blocking pool where ~100ms of KDF never parks an async
  worker. Zod's exact 400 messages ride through verbatim (probed against the ui's own zod
  4.3.6 — the email pattern hand-rolled, and its truth table pinned), the login brakes
  share one Redis counter space with TS (a Rust attempt and a TS attempt spend the same
  budget; a success resets it), and the claim's advisory lock still makes a lost race a
  409. Verified against the live dev stack: providers byte-identical between runtimes; a
  Rust login verifying a node-hashed entry and returning the login body byte-identical to
  TS's; either runtime's logout destroying the other's session; the claim limiter's 429
  (message and Retry-After) agreeing across the shared counter; every validation error
  path diffed equal.
