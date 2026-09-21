---
name: judge-pr
description: Judge a change against this repository's standards and against the application itself — the diff-level checks in scripts/judge-pr.mjs, and the reading no script can do. Use when reviewing a pull request, before asking for review, or when a change needs to be measured against what this repo says about itself.
---

# Judge a pull request

Two halves, and the split is the point.

- **`scripts/judge-pr.mjs` measures the diff.** Flow, changelog claim versus surfaces touched,
  tests, generated trees, commit subjects, the pull request body. It cannot be argued with (it
  prints the lines it judged) and it cannot see whether the change *works*.
- **You read the change against the application.** That is the rest of this file, and it is why
  a judge is a person or an agent rather than a script.

Neither half replaces the gates. A judge measures; `bun run verify` and CI block.

## 1. Run the measurable half first

```bash
node scripts/judge-pr.mjs --base origin/rc                    # what a PR against rc would get
node scripts/judge-pr.mjs --base origin/main --target rc      # a branch forked off main
node scripts/judge-pr.mjs --base origin/rc --pr-body body.md   # include the PR body
```

Findings are graded `must` (the repo states it about itself — exit 2), `should` (a convention
with a reason) and `ask` (a question for the author). A `must` is either fixed or argued in the
pull request; the other two are never a reason to hold a change. `--json` if you want the
findings as data.

It reads the diff of a commit range, so it judges what is COMMITTED: run it after `git commit`,
or pass `--head` a ref that contains your work. (Same reason it is a pull-request check and not
part of `bun run check` — there is no range to judge before there is a commit.)

## 2. Read the change against the application

The diff tells you what moved. These tell you whether it is *right here*. Each item names where
the answer lives — read that, do not guess:

| Ask | Where the answer is |
|---|---|
| Does it respect the layering — gateway for every model call, engine in the api, surface in the app? | [`docs/ARCHITECTURE.md`(../../../docs/ARCHITECTURE.md), [`docs/RUST-MIGRATION.md`(../../../docs/RUST-MIGRATION.md) |
| Does it invent a second way to do something that already has a home (a predicate, a fetch helper, a UI primitive)? | the invariant rules in [`scripts/check-invariants.mjs`](../../../scripts/check-invariants.mjs) — they name the canonical homes |
| Does a wire change match its documentation, and is the generated reference regenerated? | [`docs/API-CONVENTIONS.md`(../../../docs/API-CONVENTIONS.md), [`docs/api/`(../../../docs/api/README.md) |
| Is the UI built from the primitives, in the house idiom? | [`docs/UI-CONVENTIONS.md`(../../../docs/UI-CONVENTIONS.md) |
| Can this migration ship with the code — does anything the OLD code reads get dropped or rewritten? | [`docs/UPDATES.md`(../../../docs/UPDATES.md) — the overlap contract |
| Does it touch a secret, a key, a permission or an agent credential boundary? | [`docs/ENCRYPTION.md`(../../../docs/ENCRYPTION.md), [`docs/PERMISSIONS.md`(../../../docs/PERMISSIONS.md), [`docs/AGENT-KEY-MIGRATION.md`(../../../docs/AGENT-KEY-MIGRATION.md) |
| Are the guardrails intact — no forced `done`, no self-assignment, human sign-off where a human belongs? | [`CONTRIBUTING.md`(../../../CONTRIBUTING.md), [`docs/ARCHITECTURE.md`(../../../docs/ARCHITECTURE.md) |
| Does it claim something the code does not do — a fallback with no path to it, a feature that is dormant, a TODO wearing a doc? | the diff itself: a claim in prose needs a call site, and a call site needs a caller |
| Does it work? | run it: `bun talaria dev`, exercise the surface, or `node scripts/deploy-smoke.mjs` for the image |

## 3. Say what you could not know

The third answer — after "this is wrong" and "this is fine" — is **"I could not tell, and here
is what I would need"**. A judge that invents findings is worse than no judge: it costs the
author a round trip and teaches them to ignore the verdict. So:

- Quote the line you are judging. A finding without a location is an opinion.
- Separate "this is wrong" from "this is not my preference". The second one is not a finding.
- If the honest answer is "the verification claim does not cover the risk I can see in the
  diff", say that sentence exactly — it is the most useful thing a reviewer can write, and it is
  invisible to every gate.
- Rank by what a user would hit, not by how the code reads.

## 4. The one thing the gates cannot check

Standards are measurable; judgement is not. The mechanical half is the script's, the reading is
yours, and the repository's own test for both is the same: **would this change stand up if a
stranger read it six months from now, with only the diff, the changelog entry and the pull
request body?** If the answer needs a conversation to be true, the conversation belongs in the
pull request.