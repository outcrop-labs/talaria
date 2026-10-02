- **A Developer Agent can code on your own subscription.** Sign an agent in to
  a coding account — Claude Pro/Max, ChatGPT Codex, GitHub Copilot, Gemini,
  Cursor, Z.AI, xAI, Kimi, Perplexity and the rest of Oh My Pi's roster, 23
  OAuth sign-in flows in all — and its workbench harness runs on that account
  instead of the org's Talaria gateway. One account per service, per agent,
  signed in by the agent's own managers — the same people who can change its
  soul or its secrets, not every holder of `agents.manage`, because a
  fleet-wide permission is not a licence to put a subscription behind somebody
  else's agent. Off until an admin turns it on, and then only for the
  services the org allowlists: "developers may use coding subscriptions" and
  "may use anything omp supports" are different decisions.

  **The agent itself is untouched.** The persona driving the harness keeps the
  model its agent def configures, through the gateway, metered as always. This
  moves the harness's models and nothing else. The consequence worth stating
  plainly, because nothing else would say it: a harness run on a subscription
  does not pass through the gateway, so it does not land in Talaria's ledger —
  the provider's own dashboard is the record.

  **The gateway stays a choice, not just a fallback.** A plan is either a
  signed-in account or the gateway itself, and the gateway is listed beside
  them: it can be an agent's default, it has its own per-role model picks, and
  a ticket can pin it explicitly. Signing a subscription in never takes away
  the ability to say "do this one through the gateway, on the org's models".

  **A ticket can pick its own plan and model.** A subscription runs out
  mid-ticket, and the fix has to be local: the ticket strip picks which of the
  agent's plans this ticket codes on, and which model, without re-pointing the
  agent and every other ticket with it. Three layers resolve, narrowest first
  — the ticket's pin, the agent's default plan, then the org's Workbench model
  roles exactly as before. Role picks hang off the *plan* rather than the
  agent, because a plan is a coherent set of models: swapping plans swaps the
  set with it instead of leaving `smol` on the subscription that just ran out.
  `start_job` puts the resolved plan on the invocation line (`--model
  <provider>/<model>` plus inline `PI_*_MODEL`), so a pin — and an account
  signed in since the last roll — takes effect without re-rendering a
  container. `doctor` reports which plan the agent codes on.

  **Talaria holds the credentials, and omp performs the flows.** The whole
  credential, refresh token included, seals with secretbox into Postgres and
  appears in Admin → Secrets; the agent's omp reads it over omp's own
  auth-broker protocol at `/api/workbench/auth/v1/*`, scoped by the agent's own
  key, so an agent reads exactly its own accounts. Snapshots carry an access
  token with `__remote__` where the refresh token would be, which makes this
  instance the only thing that can refresh one — and `POST /v1/credential`, the
  protocol's upload, is refused by design, because a sandbox that could write
  its own credential could grant itself a subscription. A credential the
  provider declares dead is disabled with its reason and says "sign in again"
  instead of retrying for ever. The snapshot generation is bumped by a database
  trigger rather than by the write paths, so a revocation — which removes a row
  rather than touching one — moves it too.

  Not one provider parameter is written down here. The flows and the refreshers
  are upstream's own, run by a new `omp-auth/` bridge the api spawns beside the
  toolkit; it publishes several versions a week, and mirroring 23 client ids and
  token endpoints would have been a mirror that rots. The bridge ships as a
  self-contained bundle because the `@oh-my-pi/pi-ai` tree it builds from pulls
  364 MB of glibc-only native binaries that this alpine image can neither hold
  nor load — none of which any OAuth flow on Linux touches, so the bundler
  substitutes stubs that throw with their own symbol name if a future release
  ever routes a login through one.

  Verified: all **23** OAuth providers driven end to end through the bridge
  (`omp-auth/scripts/probe-flows.ts`) — real device codes issued for the
  device-code and custom-device flows, real PKCE authorize URLs built for the
  authorization-code flows, and the prompt-first and poll-based custom flows
  surfacing their own questions; 23/23 reached a state a person can act on.

  Exercised against a running instance (worktree stack, real Postgres and
  Redis), which is where the two bugs this change fixes were found — both of
  them invisible to the tests. The agent panel 404s while the feature is off
  and answers once it is on; the toggle and allowlist round-trip and refuse a
  service omp does not have, by name; the roster arrives through Talaria from
  omp's own rules; a real sign-in start against ChatGPT/Codex returned a
  genuine authorize URL with the real client id and then the paste-code
  prompt, and cancelling it cleaned up; a service off the allowlist is refused
  at login start. On the broker: the snapshot authenticates by the agent's own
  key, carries `__remote__` in the refresh slot with the real refresh token
  appearing **zero** times in the body, holds only the keys omp's schema
  accepts, answers 304 to a matching `If-None-Match`, and refuses a credential
  upload. A block write and clear round-trip and the database trigger moved the
  generation each time. A refresh against Anthropic's real token endpoint
  returned `invalid_grant`, which the classifier correctly treated as a dead
  grant: the row was disabled with that reason, stopped being served, and the
  panel showed it as needing sign-in while resolution fell back to the
  gateway. De-allowlisting a service emptied the snapshot and re-permitting it
  brought the credential back with no re-login. The ticket pick listed the
  agent's plans, pinned the gateway with a named model (`source` flipping to
  `ticket`), unpinned back to the agent's default, and refused a pin naming an
  account that is not that agent's.

  The two live-found bugs: the ticket's agent was looked up through a
  `tasks.assignee` column that does not exist (`assignees` is a jsonb array of
  fleet model ids, so the join is a containment test), and `open_accounts`
  decoded `disabled_at` as epoch milliseconds — which decoded fine for every
  NULL and panicked on the first row that was actually disabled. The query now
  returns the boolean the caller wants, so that type cannot drift again.

  `bun run check` green (298 routes, generated reference regenerated).
  `bun run typecheck` 0 errors. 1343 ui tests and 220 cli tests pass (the cli
  suite gained four cases for the bridge's build step, and two mcp-staleness
  assertions were tightened to name the package they were always about).
  Migration replay green from zero and idempotent on a second pass (409
  statements), with the schema snapshot regenerated; the generation trigger was
  exercised directly against Postgres for insert, update, block-write and
  delete. `cargo clippy -D warnings` and `cargo test` clean on every touched
  crate, `cargo fmt --check` clean, and `bun run gate` green. The workspace
  compile is CI's.
