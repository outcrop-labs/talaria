- **A change now reaches `main` only through a verified `rc`, and the branch
  model is enforced by code rather than agreed in prose.** Every pull request
  targets `rc`, which is both the integration branch and the staging
  environment; every push to `rc` builds this commit's image, boots it as a
  fresh instance and checks it; and `main` — the release trunk — moves one way
  only, by an automated promotion of `rc` merged as a merge commit once that
  deploy is green. Nobody opens the second pull request, and nothing reaches the
  trunk without passing the first branch's gates.

  The enforcement, three pieces sharing one implementation:

  - `scripts/flow-guard.mjs` — the policy. Content provenance, not identity: a
    push to `main` may introduce only commits already reachable from `rc`, and a
    merge's tree must BE the tree of the branch it merged (main takes `rc` whole;
    a hand-made merge with an edit smuggled inside fails) — so a direct push, a
    squash merge and a rebase merge all fail the same test, while the promotion
    merge passes it. A push to `rc` must be a merge commit or a fast-forward of
    `main`; neither may be rewritten or deleted; the retired `testing` branch
    takes no pushes and no pull requests, and deleting it is the one thing a push
    may do to it. It fails closed on an unknown — an unresolvable base, a missing
    `origin/rc`, a shallow clone — instead of passing because it could not tell.
  - `.github/workflows/flow.yml` — the server-side tripwire: a pull request to
    `main` from anywhere but `rc` is refused, the promotion's head commit must
    have a green **CI run and a green `rc-deploy` run** read from the Actions
    API, and a push to a long-lived branch is judged by the rules above. It reads
    the guard from a revision the change cannot edit (the base branch's tip; the
    tip a push moves) and refuses outright, rather than falling back to the
    pushed copy, when that revision is not in the repository at all — the
    force-push case "no guard at that revision" must never be confused with. Both
    new manual-dispatch paths are gated on their ref, because a dispatch runs the
    dispatched ref's own copy of the file with that job's token grants.
  - `scripts/hooks/pre-push` — the same policy before the network, wired by
    `talaria setup` through `core.hooksPath`, so the refusal a developer sees
    locally is the one CI would have reported.

  `rc-deploy.yml` is what "verified in RC" means: it builds the commit's api
  package and app image and runs `scripts/deploy-smoke.mjs`, which boots a fresh
  instance (scratch postgres and redis, first-boot state dir, no sidecars) and
  asserts `/api/healthz` ok — true only when the boot migration pass succeeded —
  the SPA shell on `/` and on a client route, the Rust api answering behind the
  proxy, the version reported matching the commit built, and no restart after
  all of that. `promote.yml` verifies the tip when that run finishes green and writes the
  evidence where the person who has to act reads it (the run summary); with a
  `PROMOTION_TOKEN` secret it opens the promotion pull request itself and arms
  auto-merge — only when `main`'s required checks actually name the gate, since
  auto-merge waits for required checks and nothing else — and without one it
  hands over the exact command. That split is not a preference: events caused by
  the repository's `GITHUB_TOKEN` start no workflows, so a promotion pull
  request opened with the default token would never have its checks report and
  `main`'s protection would hold it open for ever. For the same reason the merge
  is a person's (or a bot token's): a merge made with `GITHUB_TOKEN` would land
  on `main` and start nothing — not `ci.yml`, not the push guard, and not the
  trunk image feed the updater rolls from.

  One pre-existing defect surfaced on the way and is fixed here: api-package.yml
  appended the moving `main` tag to every run whose CALLER's event was a push —
  and a called workflow sees the caller's event and ref. The first push to `rc`
  (this file's own staging deploy) would therefore have re-pointed
  `ghcr.io/outcrop-labs/talaria-api:main` at an api built from `rc`, which is the
  root Dockerfile's default ARG and the digest app-image.yml pins for a main push
  that does not touch `api/` — silently baking rc's api into the trunk image the
  in-app updater rolls from. An RC tag cut on `rc` did the same. The tag is now
  conditional on the run's own ref being `refs/heads/main`, which is what the tag
  documents itself as meaning.

  `testing` is retired: the nightly channel builds `rc`'s tip, which closes the
  nightly outage the entry below records — `release.yml` was calling today's
  `ci.yml` against a branch that had sat still since 09-17, and nothing kept it
  current. Nothing auto-merges work into `rc` (a person merges; auto-merge exists
  for the promotion step alone, and only with `PROMOTION_TOKEN` configured), and
  conflicts are the author's — `flow-guard` refuses a promotion merge carrying
  content of its own so a hand-resolved conflict cannot reach `main` without
  passing through `rc`. The protection recipe needs no "Update branch" click
  either: after a promotion, `main`'s tip is the merge commit `rc` does not
  contain, so `strict` is off on both branches — the promotion is safe because
  the gate reads the runs for its head commit, not because it was tested against
  the trunk's tip. The branch model now has one home,
  [`docs/BRANCHES.md`](./docs/BRANCHES.md), with the repository settings that
  make it hold; `CONTRIBUTING.md`, `RELEASING.md`, `AGENTS.md`, the
  `ship-a-change` and `cut-release` skills, and the tooling doc all point there
  rather than restating it.

  Contributor-facing scaffolding for outside pull requests lands with it: a pull
  request template, bug and feature issue forms, [`SECURITY.md`](./SECURITY.md)
  (private vulnerability reporting — the GitHub advisory channel, no email), and
  [`CODE_OF_CONDUCT.md`](./CODE_OF_CONDUCT.md) (Contributor Covenant 2.1).

  **Not done, and it needs a human with repo settings:** branch protection on
  `main` and `rc`, merge commits only, `PROMOTION_TOKEN` (if the promotion should
  open its own pull request), "Allow auto-merge" (only alongside that token), and
  private vulnerability reporting for [`SECURITY.md`](./SECURITY.md) — all
  configured in GitHub, not in this tree. The branch-protection recipe is
  [`docs/BRANCHES.md`](./docs/BRANCHES.md) → The required settings, and until it
  is applied the guards report while somebody acts.

  Verified: 27 branch-guard scenarios in a scratch clone — a direct push to
  `main`, squash- and rebase-shaped promotions, a merge from a non-`rc` branch, a
  merge with an edit inside it, a rewrite, a delete, a direct commit on `rc`, a
  PR-shaped merge on `rc`, a trunk sync, `testing` (content refused, deletion
  allowed), tags, feature branches, bad usage, a clone with no `origin/rc` — each
  behaved as specified, and a real `git push origin main` through the hook was
  refused while a feature-branch push went through. The deploy smoke PASSED
  against a working image, FAILED on an `--expect-version` mismatch, and FAILED
  against the published `ghcr.io/outcrop-labs/talaria:main` — the 544 KB stub api
  the entry below describes, caught from outside the container exactly as a deploy
  would catch it. Both policy-materialization branches were exercised in a
  scratch clone (a base that predates the guard falls back; a base that is not in
  the repository fails the job with the reason). Every workflow YAML parses and
  all `run:` blocks pass `bash -n`; promote.yml was executed against stub
  `gh`/`git` fixtures on both paths — no token (verify, write the summary, print
  the command) and bot token (create, refresh, arm auto-merge only when
  `promotion gate (rc verified)` is among main's required contexts) — plus the
  nothing-to-promote exit. `bun run check` and `bun run verify` are green, and the
  new `branch-flow-anchors` invariant was confirmed to fail on two deliberate
  violations — `testing` restored to a trigger, and `docs/BRANCHES.md` removed.
  One test came along: `cli/src/paths.test.ts`'s `repoRoot` case asserted the
  checkout's directory NAME (`root.endsWith('talaria')`), so `bun test` in
  `cli/` could never pass in a worktree — the isolation this repo tells everyone
  to work in, at `../talaria-<name>`. It asserts the walk and its stop now
  (two levels up from `cli/src`, with a `.git` there), and the cli suite is
  187 pass / 0 fail in a worktree.
