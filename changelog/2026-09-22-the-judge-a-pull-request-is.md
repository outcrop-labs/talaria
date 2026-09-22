- **The judge: a pull request is now measured against the standards this repo
  states about itself, and against the application.** Review had no definition
  beyond "somebody read it", which is how a change with a changelog claim its
  diff does not support, a behaviour change with no test, a hand-edited generated
  reference or a commit subject off the house style all arrive looking identical
  to a clean one. `scripts/judge-pr.mjs` decides what the diff can decide —
  including the flow itself (a pull request aimed at `main` or at the retired
  `testing`) and whether the verification a change *claims* covers the surfaces
  it *touches* (`ui/` needs `verify`, `api/` needs `api:check`, `desktop/` needs
  `desktop:check`, and `cli/` is named because `verify` does not run its suite).
  Findings are graded `must` (the repo states it about itself, exit 2),
  `should` (a convention with a reason) and `ask` (a question — never a reason to
  hold a change), and every finding quotes the lines it judged.

  The half a script cannot do is a procedure now instead of a hope:
  [`.claude/skills/judge-pr`](./.claude/skills/judge-pr/SKILL.md) walks a reviewer
  through the application questions — layering and the gateway, a second copy of
  something that already has a canonical home, wire drift, migration overlap, the
  secret and permission boundaries, the guardrails — each pointed at the doc that
  answers it, with the instruction that matters most: say "I could not tell, and
  here is what I would need" rather than inventing a finding.

  `.github/workflows/judge.yml` runs it on every pull request, keeps one sticky
  comment with the verdict (edited in place, skipped on forks whose token is
  read-only — the summary still carries it) and fails the job on a `must`. It is
  deliberately NOT a required check yet: a judge with a false positive that
  blocks a merge teaches everyone to route around it, and `docs/BRANCHES.md`
  records the one-line promotion for when it has earned the trust.

  The skills index became enforced in the process: the `skills-index-drift`
  invariant fails when a skill has no row in `AGENTS.md`, a directory has no
  `SKILL.md`, or a row points at a skill that does not exist — which is what
  `docs/AGENT-TOOLING.md` had claimed since it was written.

  Verified: the judge on this very change (0 findings, exit 0) and on a planted
  range with deliberate violations — a `fleet/` file, a hand-edited
  `docs/api/README.md`, a `ui/src` change with no changelog entry and no test, an
  `Update stuff.` commit subject, an unfilled pull-request body — where each
  finding fired with the intended severity and exit 2; its own self-test caught
  two defects in it (a diff base read as a pull-request target, and a skill
  description whose trigger is on a wrapped line), both fixed; the new invariant
  fires on a removed row and on a row for a skill that does not exist, and stays
  clean with all five; `bun run check` and `bun run verify` green.
