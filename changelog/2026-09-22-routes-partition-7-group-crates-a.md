- **Routes partition: 7 group crates + a facades crate.** The 98-second
 `talaria-api-routes` unit becomes seven parallel crates; cold build
 207.3s → 183s (now faster than the pre-split monolith's 186s), routes
 edit rebuild 8.5s → 4.8s. Also repairs two latent split casualties the
 fail-fast test runner had been hiding: the toolbox census test lost its
 registry scan (moved to harness-defs next to the registry), and
 hermes-skills read `scripts/skills` off a stale relative path. The full
 suite is 2,175 tests — the earlier "868" figure was an undercount from
 aborted runs. Verified: `cargo test --workspace` (2,175 passed, exit 0),
 `clippy -D warnings`, `bun run check` (245-route table unchanged).

 117 static-init) is safe-by-construction — constant regexes, guarded
 doubles; the two fragile guarded-unwrap shapes (history `kind`, oauth
 callback tuple) are destructured instead. The workspace's one remaining
 `unsafe` (SSE metered stream `get_unchecked_mut`) is audited sound.
 Split-era helper copies deduped onto their owners. Verified:
 `cargo clippy --workspace --all-targets -- -D warnings`,
 `cargo test --workspace` (868 passed).

 mid-build at 15. Verified: the rerun on this change is the first to fit
 the budget cold.

 it. Verified: `bun run check` (doc links).

 build is cargo-chef-layered (deps build once per manifest change) and
 exports its layers through a buildx registry cache (`:buildcache`, mode=max)
 — the gha backend can't be used because release.yml calls the build via
 workflow_call. Verified: `bun run check`; the image build itself exercises
 on the next push to `main` (no docker on this box).


  the split: clippy `-D warnings` and 868 tests green. Verified:
  `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`.
