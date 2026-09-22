- **The pre-stable channels compile the api with the dev profile.** `nightly`
  and `rc` publish a package built by cargo's own dev profile
  (`api/Cargo.toml`'s `[profile.dev]`: our crates `-O1`, dependencies `-O3`,
  `debug = "line-tables-only"`) instead of release. Measured on the same box,
  same sources: the compile of our ~200 crates is **7m52s instead of 16m04s**
  (the CI runner was still going at 24m30s when its budget killed it, and that
  run published nothing), and a whole image build is 8.4 minutes with a warm
  cook instead of ~21 — the difference between a nightly that lands the same
  day and one that never lands at all.

  `release.yml`'s `resolve` decides, in one place, keyed on the channel;
  `api-package.yml` carries a `profile` input and passes it as the
  Dockerfile's `PROFILE` arg; main's feed and every stable tag keep release,
  and `PROFILE` refuses any other value by name rather than silently building
  something. `RELEASING.md` states the trade where an operator reads it: an rc
  image is a smoke test of the same sources — debug assertions ON, ~417 MB
  binary vs release's ~144 MB — not a performance preview of the `X.Y.Z` that
  is built again from the tag, in release.

  The first dev-profile build also caught a defect in #407's stub gate: under
  `set -eu` the probe aborted the subshell on an *empty first probe*, with no
  output at all — indistinguishable from the stub the gate exists to catch. It
  passed until now only because a release binary answers on the first try. The
  probe tolerates a miss (`|| true`, `if`) and keeps its 30-second patience.

  Verified: `docker build --build-arg PROFILE=dev` → `Finished dev profile in
  7m 52s` + `stub gate: the built binary answers /api/healthz (HTTP/1.1 503)`;
  `PROFILE=release` (the default) still builds release and passes the same
  gate; `PROFILE=staging` fails the build in the deps stage by name; the gate
  body, extracted verbatim and run under `set -eu`, passes a real binary
  (exit 0) and fails the published 544 KB stub by name (exit 1);
  `release.yml`'s resolve script run under all five event shapes emits
  `profile=dev` for nightly/rc (schedule, dispatch nightly, dispatch rc, `-rc.N`
  tag) and `profile=release` for `vX.Y.Z`, with a malformed tag still exiting 1;
  both workflow files parse; `bun run check` green.
