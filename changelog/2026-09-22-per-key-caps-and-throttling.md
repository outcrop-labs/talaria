- **Per-key caps and throttling** (#265): each LLM-gateway key carries its own ceilings in
  **Settings → API keys** — a spend cap (tokens/$ over the org budget window) and a
  requests-per-minute limit. Both self-imposed and unlimited by default; the cap can only
  tighten an admin's ceiling, never raise past it, and the refusal (`429 budget_exceeded` /
  `rate_limit_exceeded`, with `retry-after`) names the surface that holds the number.

### Security

- Upstream error text is sanitized at the trust boundary (#268): the two
  proxy wires — `/api/llm/v1/chat/completions` (external key holders) and the
  MCP gateway (agent containers) — no longer forward an upstream's error body
  verbatim. Failed hops answer with the status (ours to share) and a fixed
  sentence; the only upstream-written text that survives is the structured
  `error.type`/`error.code` tokens OpenAI-style clients switch on for retries,
  length-capped. The verbatim body goes to the server log, inside the
  boundary. JSON-RPC errors on the MCP wire ride 200s and pass untouched —
  tool results, including tool failures, are the protocol the agent speaks.
- Oversized uploads are refused before they are buffered (#266): the upload
  route reads its multipart body through a capped stream — a declared
  content-length over the cap is answered 413 from the header alone, and a
  chunked body (what every browser FormData POST is) is aborted mid-read at the
  cap. Previously the whole body was buffered and only saveUpload's byte count
  refused — after the memory was already spent.
- Compose generates its first-boot secrets (#267): `talaria deploy up` writes
  `POSTGRES_PASSWORD` and the minio root pair into `docker/.env` (once, 0600,
  git-ignored) before invoking compose — the two places no container entrypoint
  can reach, because interpolation happens at container-create time. An existing
  postgres volume keeps the password it was initialized with (a fresh random
  would lock the app out); the file says how to rotate. Without this, an
  unconfigured compose instance ran on passwords published in the repo.
- Password credentials live in Postgres as scrypt hashes (#244): `user_password_credentials`
  stores `scrypt$N$r$p$salt$hash` (node:crypto, params in-band) — never plaintext, never env.
  A login miss on the email burns a dummy verify, so response timing can't reveal which
  addresses have accounts.
- Google sign-in refuses unverified email addresses (#269): an identity whose email claim
  Google has not verified — including an absent claim — is rejected at code exchange, so an
  unverified address can no longer mint an account.
