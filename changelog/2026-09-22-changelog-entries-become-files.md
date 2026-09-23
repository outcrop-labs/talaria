- **Unreleased changelog entries are one file each under `changelog/`, and the
  single-section anchor is retired.** `## [Unreleased]` was the repo's worst
  merge-conflict surface — CHANGELOG.md is the most-touched file in the
  tree, every open PR landed at the same anchor, and merges reconciled
  bullets by hand (three same-day PRs demonstrated it again this week). An
  entry now lands as `changelog/YYYY-MM-DD-<slug>.md` — the verbatim bullet,
  bold lead and Verified: paragraph — and `bun scripts/changelog-roll.mjs
  <version>` folds them into a version section at release time and deletes
  the files, so CHANGELOG.md itself stays append-only, written only by the
  roll. `--check` is the new link in `bun run check`: entry files must be
  well-formed and a hand-appended `[Unreleased]` bullet fails with the
  filename recipe; judge-pr flags the same from the diff side, and the
  Verified: convention is enforced there, where entries are new. The 314
  entries then live in `[Unreleased]` migrated verbatim (frozen history is
  not rewritten to satisfy the newer convention). ship-a-change and
  cut-release gained the roll step before the tag.
  Verified: `--check` green over 314 migrated entries; the roll exercised
  for real (314 entries folded under a test heading, then restored) and
  re-green after; `bun run check` green end-to-end with the new link;
  judge-pr's changelog arm reads added `changelog/` files and flags
  CHANGELOG-appended bullets (node --check + the judge run on this very
  branch, whose entry is this file).
