- **New: a release is two merges. CI rolls the changelog, tags the promotion and writes the release notes.** When rc is verified and `changelog/` has entries, `promote.yml` opens the roll's pull request against rc (branch `release/roll`) at the version `scripts/release-version.mjs` works out. That's the next patch, unless an entry asks for more with a `<!-- bump: minor -->` line or a `version` dispatch sets it. Once the roll merges and rc verifies again, it opens `promote: rc → main — vX.Y.Z`. That merge makes the new `tag-release.yml` tag main and dispatch `release.yml` at the tag: a tag pushed with the workflow token starts no workflow. An entry that lands after the roll extends the same section, and an open promotion is held until that roll merges. The GitHub Release body is no longer a stub. `scripts/release-notes.mjs` builds it from the section:
  - the images, the newest Desktop release and the pull requests;
  - **Before you upgrade**: the migration count (from `MIGRATIONS` at the previous tag and this one) and each entry's new optional `Upgrade:` paragraph;
  - each entry's lead, then the full section.

  The roll and promotion pull requests need a `PROMOTION_TOKEN` (Contents and Pull requests write). Without one, the run prints the commands instead. The tag and the release need no token.

  Verified: `scripts/release-chain.test.mjs` (19 checks, in `bun run check`) covers the version arithmetic, `--extend`, bump markers, and the notes' sections, counts and body limit. Notes built for 0.13.2 from real history name #713, #716, #718, #719 and #720, with no migrations. The migration count reads 555 → 558 across v0.13.0 → v0.13.1, matching outcrop's 558 applied. Dry runs used `promote.yml`'s and `tag-release.yml`'s own step scripts against a scratch origin with `gh` stubbed:
  - no token: a notice to roll v0.14.0 when an entry asks for a minor;
  - with a token: the roll is pushed, and an unchanged tip is left alone;
  - dispatches: 1.0.0 is taken and 0.13.2 refused;
  - after the roll merges: the promotion is titled v0.14.0;
  - a late entry with a promotion open: the section is extended and the promotion held;
  - the promotion merge: the tag is cut and `release.yml` dispatched, and a re-run does nothing.

  `release.yml`'s resolve was run for dispatch-at-tag, branch dispatches, RC tags and a malformed tag. Not run on GitHub itself until the next release.
