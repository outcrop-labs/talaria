- **README, resynced to main — the promotion guard reads the tree, and main's tree had drifted.**

`flow.yml`'s `branch push (main takes rc content only)` job started failing on
every main push (runs 41 and 46, `Rc (#428)` and `Rc (#431)`): the
`flow-guard.mjs` provenance rule reads **content, not commit identity** — a
promotion merge's tree must equal the tree of the rc tip it merges. It caught a
real drift, not a false positive.

## What actually happened

The guard's check `this merge's tree is not the tree of <merge>^2` reproduces
locally, byte for byte:

```
node scripts/flow-guard.mjs push refs/heads/main 7e159bfa… 20366a14…   # BLOCKED
```

Mechanism: PR #425 ("Revise README.md for improved clarity and detail") was
squash-merged into **main** on Sep 22 (44e0af7a). A squash commit exists nowhere
else, so the #425 revision became main-only content. The next promotion merge
(#431) kept that tree, and rc never received the README change — so
`tree(promotion) != tree(rc tip)` on every subsequent promotion push. The guard
naming the merge as a "smuggled edit" is its designed response to exactly this
shape.

Notably, #425 predates the guard entirely (the guard shipped in #411's era but
the README revision went through a squash to main before/around the guard's
activation) — so the tree drifted before the tripwire existed, and only the
guard's new tree-comparison half could see it.

## The fix

Exactly what the guard's own refusal message prescribes: land the difference on
rc. This PR takes main's README blob (`73521cc…`, identical to #425's content)
and commits it on a branch from current rc, so the next promotion merge
carries matching trees again and the guard passes on its own terms. No guard
code is touched — the guard was right.

## Verification

- `diff(3ed2fd2f:README.md, 44e0af7a:README.md)` = the whole 279-line #425
  revision; after this PR, `tree(rc tip)` contains it and the next promotion's
  `tree(promotion) == tree(rc tip)` by construction.
- `bun run check` green (invariants 814 files, 15 rules; docs 470 files / 735
  links; generated references current; changelog well-formed).
- `bun run verify` green (typecheck + tests + invariants + reference drift) —
  README-only change, all suites pass.
- CHANGELOG entry added per CONTRIBUTING (doc-facing fix, changelog entry
  riding with the change).
