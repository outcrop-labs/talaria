- **Handlers answer `Result<Response, Response>`, and the gate/body/secretbox
  unwraps collapse onto one call each.** Five passes over the route crates,
  every one locked by a new invariant rule:

  * **Gates propagate with `?`.** ~380 call sites spelled out
    `match require_user(&state, &headers).await { Ok(u) => u, Err(gate) => return
    gate }`. Every guard already returns `Result<_, Response>`, and axum
    implements `Handler` for any `R: IntoResponse` — `Result<Response, Response>`
    is one — so the whole match becomes `require_user(&state, &headers).await?`.
    `rust-hand-wrapped-gate` fails the next one.
  * **`object_or_400`.** 194 route files hand-wrote the same 400 for a
    non-object body. `let obj = object_or_400(&parsed)?;` is that conversion,
    defined beside the envelope it produces (`talaria_body` stays pure and
    answers the message only). `rust-as-object-unwrap` fails the next one.
  * **`secretbox_or_500`.** 29 `state.secretbox()` hand-unwraps become
    `secretbox_or_500(&state, "<context>").await?`. It lives in `talaria-session`
    rather than `talaria-error` — `AppState` depends on the error crate, so the
    cycle the plan assumed away is real; session is the layer that may name both
    `AppState` AND a `Response`. `rust-secretbox-unwrap` fails the next one.
  * **In-crate gates get one home each.** `channel_gate` (6 copies in comms),
    `edit_gate` (boards), `owner_gate` (teams) and `can_manage_agent` (fleet) now
    live in their module's `mod.rs`; `talaria_params::uuid_gate` absorbed its 4
    copies and gained `uuid_gate_404` for the one route that answers "not found".
  * **`api/tests/support/`.** An integration test is its own crate, so the
    shared fixtures HAD to be copied per binary — 19 `pool()`/`pg()`, 5
    `fabricate_user`, 3 `person`, 3 `sweep_user_rows`, 3 `app_state` and the
    same DATABASE_URL expect string in 25 files. The prefix variation is a
    parameter now. (The 13 `cleanup` helpers stayed put: each holds a different
    table and WHERE clause, so they were never copies.)

  **The docs oracle moved one row group, on purpose.** `scripts/gen-docs.mjs`
  now follows helpers in the module's `mod.rs` (the gate move would otherwise
  drop every 403 from the reference) and reads the new `object_or_400` binding.
  That surfaced a pre-existing inaccuracy it had never been able to see: the
  three `/api/teams/{id}*` GET rows are `session + view:/teams`, because
  `reader_gate` consults `require_view` for a non-member. `docs/api/**` is
  regenerated with exactly those 6 rows changed; every path, method, body field,
  return shape and status list is byte-identical.

  Verified: `bun run api:check` (fmt + clippy `-D warnings` + `cargo test
  --workspace`, exit 0), `bun run check` (13 rules clean, `gen-docs --check`:
  245 routes, no further drift). The `#[ignore]`d live-DB suite was **not**
  executed — this host has no Postgres, Redis or docker (the dev stack cannot be
  started here), so `api/tests/support/` is compile- and clippy-verified only.
