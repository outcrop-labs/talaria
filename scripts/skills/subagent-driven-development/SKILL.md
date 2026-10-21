---
name: subagent-driven-development
description: "Execute a multi-task implementation plan through one workbench job: a fresh subagent per task driving the coding harness, with two-stage review (spec, then quality) after every task."
version: 2.0.0
author: Talaria (structure adapted from obra/superpowers via Hermes Agent)
license: MIT
platforms: [linux, macos, windows]
metadata:
  hermes:
    tags: [delegation, subagent, implementation, workflow, workbench]
    related_skills: [workbench-driving, oh-my-pi, requesting-code-review, test-driven-development]
---

# Subagent-Driven Development

## Overview

Execute an implementation plan by dispatching a fresh subagent per task, each one **driving the
workbench's coding harness** for its task, with a systematic two-stage review after every task.

**Core principle:** Fresh subagent per task + the harness does the writing + two-stage review
(spec, then quality) = high quality, fast iteration, nothing hand-coded.

**This is Talaria.** Nobody in this flow writes source files — not you, not a subagent. Code is
written by Oh My Pi inside a workbench job (`start_job` → the harness runs in the `workdir` →
`finish_job`); the **workbench-driving** skill is the discipline and the **oh-my-pi** skill is the
harness. A subagent here is a *driver* of that harness for one task, never an editor with a file
tool. Developer Agents only: an agent without the workbench server has no job to start, and a
coding ask is `report_gap`, not something to do by hand.

## When to Use

Use this skill when:
- You have a ticket whose work splits into several mostly independent tasks
- Quality and spec compliance matter
- You want automated review between tasks instead of one review at the end

**vs. driving the whole ticket yourself in one session:**
- Fresh context per task (no confusion from accumulated harness output)
- Review after each task catches drift before it compounds
- Consistent checks across all tasks

## The Process

### 1. Open the job and write the plan

The plan is the `plan` argument `start_job` already requires for `standard` and `heavy` effort
(the workbench-driving gates): the approach, the files you expect to touch, and how you will test.
Write it before the job, from the ticket and the code:

1. `get_ticket` — read it like an engineer. Ambiguous scope is a comment on the ticket asking,
   not a guess.
2. `start_job` with the ticket, the repo, the effort and the plan. Clone into the `workdir` it
   returns, on the `branch` it names, then `prepare_env` with the jobId. Read the `guide`.
3. Look at the code in the workdir before you split the work. A plan that names real files beats
   one that names hopes.
4. Split the plan into tasks (2–5 minutes of harness work each, below) and write them down with
   `todo` so the person watching sees where you are:

```python
todo([
    {"text": "Task 1: User model with email + password_hash", "status": "pending"},
    {"text": "Task 2: password hashing helper", "status": "pending"},
    {"text": "Task 3: login endpoint", "status": "pending"},
])
```

**Key:** ONE job, ONE workdir, ONE branch for the whole plan. Subagents run inside it, one at a
time — the harness keeps one session per workdir, and two drivers on it at once corrupt each
other's turns. Extract every task's full text up front; never make a subagent re-read the ticket.

### 2. Per-Task Workflow

For EACH task in the plan:

#### Step 1: Dispatch an Implementer Subagent — a harness driver

Use `delegate_task` with complete context. The subagent gets the terminal because the harness is
a command line; it does **not** get a file tool, because it does not write files:

```python
delegate_task(
    goal="Drive Oh My Pi to implement Task 1: User model with email and password_hash",
    context="""
    YOU DO NOT WRITE CODE. You drive the coding harness in an existing workbench job and
    read what it did. Never edit a file yourself, never create one, never `git add -A`.

    JOB (already open — do not start_job, do not finish_job):
    - workdir: /opt/data/workbench/<jobId>/repo      (cd here for every command)
    - branch:  omp-tala35                             (already checked out)
    - first turn:    <the `jsonRun` line from start_job's answer, verbatim>
    - later turns:   <the `continueJsonRun` line, verbatim — same --session-dir>

    TASK FROM PLAN:
    - Create src/models/user.py: a User class with email (str) and password_hash (str)
    - bcrypt for hashing; __repr__ for debugging
    - Tests in tests/models/test_user.py

    DRIVE IT LIKE THIS (TDD, one scoped ask per turn):
    1. First turn: "Write a failing test in tests/models/test_user.py for a User model
       with email and password_hash. Run it; it must fail." Read the JSON result.
    2. Continue: "Implement the minimal src/models/user.py so that test passes. Run it."
    3. Continue: "Run the whole suite: pytest tests/ -q. Report."
    4. Read `git diff` yourself. If the diff does more than the task, steer it back.
    5. Commit by explicit path: git add src/models/user.py tests/models/test_user.py
       && git commit -m "feat: add User model with password hashing"

    PROJECT CONTEXT:
    - Python 3.11, Flask app in src/app.py; existing models in src/models/
    - Tests use pytest from the repo root; bcrypt is already in requirements.txt

    REPORT BACK: what the harness changed (file list), the test command and its
    result verbatim, the commit hash, and any question you could not answer.
    """,
    toolsets=['terminal']
)
```

The harness lines come from `start_job`'s answer and are run as given — never swap `--model`,
never `--no-session`, never the TUI (the oh-my-pi skill). Git over https works in the workdir
with no setup; never `gh`, never a token in a URL.

#### Step 2: Dispatch a Spec Compliance Reviewer

After the implementer reports, verify against the original task text. Reviewers read; they do
not change anything:

```python
delegate_task(
    goal="Review whether the Task 1 implementation matches its spec",
    context="""
    Read-only. Work in /opt/data/workbench/<jobId>/repo. Use `git show <hash>` and read files;
    change nothing.

    ORIGINAL TASK SPEC:
    - src/models/user.py with a User class: email (str), password_hash (str)
    - bcrypt for password hashing; __repr__

    CHECK:
    - [ ] Every requirement in the spec implemented?
    - [ ] File paths match the spec?
    - [ ] Signatures and behaviour match?
    - [ ] Nothing extra added (no scope creep)?

    OUTPUT: PASS, or the specific spec gaps to fix.
    """,
    toolsets=['file']
)
```

**If spec issues are found:** dispatch the implementer again (a fresh one, with the gaps as its
task — still driving the harness), then re-run the spec review. Continue only on PASS.

#### Step 3: Dispatch a Code Quality Reviewer

After spec compliance passes:

```python
delegate_task(
    goal="Review code quality of the Task 1 implementation",
    context="""
    Read-only, in /opt/data/workbench/<jobId>/repo.

    FILES TO REVIEW:
    - src/models/user.py
    - tests/models/test_user.py

    CHECK:
    - [ ] Project conventions and style?
    - [ ] Error handling?
    - [ ] Names that say what things are?
    - [ ] Test coverage of the edges?
    - [ ] No obvious bugs, no security issues?

    OUTPUT FORMAT:
    - Critical Issues: [must fix before proceeding]
    - Important Issues: [should fix]
    - Minor Issues: [optional]
    - Verdict: APPROVED or REQUEST_CHANGES
    """,
    toolsets=['file']
)
```

**If quality issues are found:** a fresh implementer drives the fix through the harness;
re-review. Continue only on APPROVED.

#### Step 4: Mark Complete

Move the `todo` marker and go to the next task.

### 3. Final Review

After ALL tasks are complete, dispatch a final integration reviewer (read-only) over the whole
branch: do the pieces work together, any inconsistencies between tasks, is the test suite green,
is it ready for a human to review? Use the **requesting-code-review** dimensions.

### 4. Verify and finish the job

You own the result, not the harness and not the subagents. In the workdir:

```bash
git log --oneline <base>..HEAD      # every commit is a task you reviewed
git diff <base>..HEAD --stat        # only what the ticket asked for
<the repo's test command>           # green, run by you
```

Anything with a front end: drive it in a real browser (`npx playwright`) and keep the
screenshot for the ticket. Then `finish_job` with a summary that says what changed, how you
verified it, and what the reviewer should look at hardest — Talaria checks the branch and opens
the pull request. A job that produced nothing is `finish_job` with `abandon:true`, never a
hand-written substitute.

## Task Granularity

**Each task = 2–5 minutes of harness work.**

**Too big:**
- "Implement user authentication system"

**Right size:**
- "User model with email and password fields"
- "Password hashing function"
- "Login endpoint"
- "JWT token generation"
- "Registration endpoint"

## Red Flags — Never Do These

- Start without a job (`start_job`) or without the plan it requires
- Give an implementer subagent a file tool, or let one edit a file "just this once"
- `git add -A` / `git add .` — the workdir holds `.env` and caches `prepare_env` dropped there;
  stage by explicit path
- Run two implementers at once on the same workdir
- Skip either review (spec compliance OR code quality)
- Proceed with unfixed critical/important issues
- Make a subagent read the ticket or the plan (provide the full task text in context)
- Skip scene-setting context (the subagent must know where the task fits and which lines run
  the harness)
- Ignore subagent questions (answer before letting them proceed)
- Accept "close enough" on spec compliance
- Let the implementer's own report replace the review (both are needed)
- Start the quality review before the spec review is PASS (wrong order)
- Fix something by hand in your own session because the harness is being slow — that is a
  `report_gap` if the harness cannot do it, and a better-scoped turn if it can

## Handling Issues

### If a subagent asks questions

- Answer clearly and completely; add context if needed
- Do not rush it into the turn

### If a reviewer finds issues

- A fresh implementer drives the fix; the reviewer reviews again
- Repeat until approved; never skip the re-review

### If a subagent fails a task

- Dispatch a new implementer with specific instructions about what went wrong (the harness
  session is still there: `continueJsonRun` picks up where it left off)
- Do not try to fix it manually in the controller session (context pollution, and hand-coding)

### If the harness cannot do the work

- Missing toolchain or system package: add it to the repo's `mise.toml` or
  `.talaria/workbench.toml` as part of the change and `prepare_env` again
- Anything else: `report_gap` once, block the ticket with the reason, and stop

## Efficiency Notes

**Why a fresh subagent per task:**
- No context pollution from earlier tasks' harness output
- Each driver gets a clean, focused brief

**Why two-stage review:**
- Spec review catches under/over-building early
- Quality review ensures the implementation is well built
- Catches issues before they compound across tasks

**Cost trade-off:**
- More subagent invocations (driver + 2 reviewers per task)
- But issues are caught early (cheaper than debugging compounded problems later)

## Integration with Other Skills

### With workbench-driving and oh-my-pi

This skill is a way of running ONE workbench job: the gates (`plan`, one workspace, the live-job
cap, `prepare_env`) and the finishing discipline are workbench-driving's; how a turn is phrased
and continued is oh-my-pi's. Each implementer subagent is one driver of that same session.

### With test-driven-development

Every implementer brief asks the harness for the failing test first, then the minimal
implementation, then the suite. Include those three turns in every brief.

### With requesting-code-review

The two-stage review process IS the code review. For the final integration review, use the
requesting-code-review skill's review dimensions.

### With systematic-debugging

If the harness hits a bug mid-task: steer it through the systematic-debugging process (root
cause before fix, regression test), then resume the task.

## Example Workflow

```
[get_ticket TALA-35; plan written; start_job (standard, plan) → workdir, branch, jsonRun line]
[clone, prepare_env; todo with 5 tasks]

--- Task 1: User model ---
[Dispatch implementer (harness driver)]
  Driver: "Should email be unique?"
  You: "Yes, email must be unique"
  Driver: omp wrote src/models/user.py + test; 3/3 passing; committed a1b2c3d (by path).

[Dispatch spec reviewer]
  Spec reviewer: PASS — all requirements met

[Dispatch quality reviewer]
  Quality reviewer: APPROVED — clean code, good tests

[Mark Task 1 complete]

--- Task 2: Password hashing ---
[Dispatch implementer]
  Driver: no questions; omp implemented it; 5/5 passing; committed.

[Dispatch spec reviewer]
  Spec reviewer: Missing: password strength validation (spec says "min 8 chars")

[Dispatch a fresh implementer with the gap]
  Driver: continueJsonRun — validation added; 7/7 passing; committed.

[Spec reviewer again]
  Spec reviewer: PASS

[Dispatch quality reviewer]
  Quality reviewer: Important: magic number 8, extract to a constant
  Driver (fresh): omp extracted MIN_PASSWORD_LENGTH; committed.
  Quality reviewer: APPROVED

[Mark Task 2 complete]

... (continue for all tasks)

[Final integration reviewer over the branch]
[You: git diff --stat, full suite green, browser check for the UI task]
[finish_job → PR URL into report_outcome]
```

## Remember

```
One job, one workdir, one branch
Fresh subagent per task — a driver, never an editor
The harness writes; you read, steer, verify
Spec compliance FIRST
Code quality SECOND
Never skip reviews
Stage by path, never -A
```

**Quality is not an accident. It's the result of systematic process — and in Talaria, of never
hand-coding around the process.**

## Further reading (load when relevant)

When the orchestration involves significant context usage, long review loops, or complex
validation checkpoints, load these references for the specific discipline:

- **`references/context-budget-discipline.md`** — Four-tier context degradation model (PEAK /
  GOOD / DEGRADING / POOR), read-depth rules that scale with context window size, and early
  warning signs of silent degradation. Load when a run will clearly consume significant context
  (multi-phase plans, many subagents, large artifacts).
- **`references/gates-taxonomy.md`** — The four canonical gate types (Pre-flight, Revision,
  Escalation, Abort) with behavior, recovery, and examples. Load when designing or reviewing any
  workflow that has validation checkpoints — use the vocabulary explicitly so each gate has
  defined entry, failure behavior, and resumption rules.

Both references adapted from gsd-build/get-shit-done (MIT © 2025 Lex Christopherson).
