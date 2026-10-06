- **Every cross-check test in the api was failing, and none of them was being
  run.** CI's api job runs `cargo test` from `api/`, and because
  `api/Cargo.toml` is a workspace root that *also* carries a root package with
  no `default-members`, bare `cargo test` selects only `talaria-api` — three
  test binaries, none of the ~200 crates under `crates/`. Their test code was
  not merely unrun, it was never compiled: an outright type error inside a
  member crate's `#[cfg(test)]` module passes both CI steps, while the same code
  scoped to its own crate is a hard `E0308`. (Lint coverage is fine —
  `clippy --all-targets` does reach the member crates' library code.)

  Nine tests were failing on `rc`, and **every one of them was a cross-check** —
  the kind whose entire job is to notice that somebody added a thing and did not
  register it. So nothing in the tree was cross-checking anything. Three root
  causes:

  **`hermes:work` was added to the harness registry and reached three places out
  of four.** `registry.rs`'s `EXPECTED_IDS`, its subject-of-call list, and
  `score.rs`'s fitness slot bindings all missed it — so the harness was
  *measured, scored, archived and shown nowhere*, which is verbatim what
  `score.rs`'s own test comment says it exists to prevent. It declares
  `requires: ["tools", "tool-select"]`, which is the Agent slot's own
  requirement, so it now sits beside `hermes:governance` in `DEFS` order.

  **Four Google Slides and Sheets tools were registered in the toolkit and
  invisible to model fitness.** `read_google_slides`, `read_google_sheet`,
  `update_google_slides` and `update_google_sheet` had no catalogue entries;
  their descriptions are now the registrations' own, verbatim, because the
  sibling test pins exactly that and a copy that drifts measures a prompt the
  agent never saw. `move_board_to_team`'s copy *had* drifted — the toolkit grew
  a sentence about the destination team and the catalogue kept the old text.
  The Sheets pair also had no sandbox backend, so the chain registered ↔
  catalogued ↔ backed ↔ exercised could not close; they are modelled line for
  line on their Slides siblings, with a toolbox test driving them rather than a
  new entry on a harness's curated dry-run surface, which would have changed
  what that harness's fixtures measure.

  **The Hermes Google harness had grown from 13 fixtures to 17** with its count
  and band assertions never updated, and five of the newer fixtures had no arms
  in the test's `good_calls_for`/`good_answer_for` helpers. Four tolerated the
  default; one could not — its check refuses outright without a
  `create_google_doc` call, so its own good answer could never pass, which
  measures the fixture rather than the model. The queued-write trio needed the
  same treatment from the other direction: their checks require both the call
  *and* an answer that says the change has not happened yet.

  Two test names were lying and are renamed rather than left:
  `eleven_fixtures_across_three_bands` asserted 13, and
  `the_catalog_carries_sixty_one_distinct_tools` asserted 77. A number in a test
  name is a number nobody updates.

  Verified: `bun run gate` green — it was failing on these before. Per crate:
  `talaria-harness-defs` 435 passed (was 432/3), `talaria-fitness` 416 passed
  (was 415/1), `talaria-fitness-talaria-tools` 9 passed (was 7/2),
  `talaria-fitness-toolbox` 72 passed (was 70/1). clippy clean across all four.
  The dark window and the CI selection behaviour were established empirically —
  `cargo test --no-run` listing three binaries, and planted probes proving a
  member crate's test code is neither linted nor compiled.
