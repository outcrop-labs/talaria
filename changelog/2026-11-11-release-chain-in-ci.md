- **New: a release is two merges. CI rolls the changelog, tags the promotion and writes the release notes.** When rc is verified and `changelog/` has entries, `promote.yml` opens the roll's pull request against rc (branch `release/roll`) at the version `scripts/release-version.mjs` works out:
  - the next patch by default;
  - more when an entry asks with a `<!-- bump: minor -->` line;
  - never below an RC already tagged past the newest stable;
  - or a `version` dispatch, which sticks until that roll merges.

  Once the roll merges and rc verifies again, it opens `promote: rc → main — vX.Y.Z`. That merge makes the new `tag-release.yml` tag main and dispatch `release.yml` at the tag, because a tag pushed with the workflow token starts no workflow. A re-run re-dispatches a tag that has no Release.

  Every promotion is a release now. `flow.yml`'s promotion gate fails while `changelog/` has entries, and `tag-release.yml` won't tag a main that carries any. An entry that lands after the roll extends the same section, and the roll refuses to open a second untagged section or to extend a tagged one.

  The GitHub Release body is no longer a stub. `scripts/release-notes.mjs` builds it from the section, plus a stable's RC sections:
  - the images, the newest Desktop release and the pull requests;
  - **Before you upgrade**: the migration count (from `MIGRATIONS` at the previous tag and this one) and each entry's new optional `Upgrade:` paragraph;
  - each entry's lead, then the full text.

  A dispatch at an older stable tag is refused rather than moving `latest` back. The roll and promotion pull requests need a `PROMOTION_TOKEN` (Contents and Pull requests write). Without one, the run prints the commands instead; the tag and the release need no token.

  Verified: `scripts/release-chain.test.mjs` (23 checks, in `bun run check`) covers:
  - the version arithmetic: RC floor, indented and malformed markers, overrides;
  - `--extend` and the roll's pending guard in a real git repo;
  - the notes' sections, RC folding, counts and body limit.

  Notes built for 0.13.2 from real history name #713, #716, #718, #719 and #720, with no migrations. The migration count reads 555 → 558 across v0.13.0 → v0.13.1, matching outcrop's 558 applied.

  Dry runs used the workflows' own step scripts against a scratch origin with `gh` stubbed:
  - promote: no token; a default minor roll; a 1.0.0 dispatch that survives the next run with no re-push; a late entry that extends 1.0.0 and holds the open promotion; the roll merged, giving a promotion titled v1.0.0 and the stale roll pull request closed;
  - the gate: entries present fails, a 404 passes, a 502 fails;
  - tag-release: a leftover entry is refused; a failed dispatch goes red, then is re-dispatched on the re-run; nothing happens once the Release exists;
  - `release.yml`'s resolve: dispatch at the newest tag, at an older one (refused), at an RC, and on a branch.

  Not run on GitHub itself until the next release.
