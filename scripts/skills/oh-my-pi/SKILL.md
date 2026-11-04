---
name: oh-my-pi
description: Drive Oh My Pi (omp), the workbench's coding harness, through a Talaria workbench job as an orchestrator: scoped asks, same session, continue with -c. Use for every coding job.
---

# Oh My Pi (Talaria)

You are the orchestrator. Oh My Pi (omp) is the pair programmer, and the only
coding harness a Developer Agent has. Do not hand-code. Do not one-shot the
ticket.

## Path

1. `start_job` with the ticket, repo, effort, and (for standard/heavy) a plan.
   Talaria then **provisions the job before you touch it**: it clones the branch
   into the `workdir`, installs the repo's toolchain, installs the pinned Oh My
   Pi into a harness dir of this job's own, and checks it (the launcher starts,
   its dir is writable, the model is in its catalog, the gateway accepts the
   key). The answer's `provision` says how that went.
   - `provision.state: ready` — the `harnesses[0]` lines are there. Run them.
   - `running` — poll `job_status` until it is `ready`. Do nothing else to the job meanwhile.
   - `failed` — the step and its output are in the answer and on the ticket.
     It is the platform's to fix, not yours: `report_problem` with them,
     comment, block. (A `toolchain` failure caused by what the repo declares is
     the one exception: fix the declaration, then `prepare_env`.)
2. Never clone, install, upgrade, wrap or reconfigure the harness yourself, and
   never reuse a harness dir, wrapper script or key from another job or an
   earlier session — the launcher in the lines is the whole command.
3. First turn: `jsonRun` (else `run`). One scoped ask. Fill `<task>`. It runs
   from any directory: the launcher stands in the workdir and loads the
   project env.
4. Run every harness turn in the **background** (your terminal's background
   mode) and wait on the process. A foreground call is cut off at the
   terminal's timeout, which kills omp mid-edit.
5. Later turns: `continueJsonRun` / `continueRun` (`-c`, same `--session-dir`).
   Read the JSON events (one per line). Steer. Never `--no-session`, never the
   TUI. `--auto-approve` is already on the line so tool calls do not hang.
6. Hash-anchored edits, LSP, subagents, and a browser live *inside* omp.
   Do not reimplement them.
7. Git over https:// just works. Never `gh`, never a token in a URL.
8. `git diff`, verify, `finish_job`.

`doctor` is for when something about the workbench looks wrong outside a job;
a job does not need it first.

## Models

omp picks its own models from the org's Workbench roles: `--model` on the
line is its default, and it moves to a fast model for subagents and small
steps and a reasoning model for planning and review on its own. Run the lines
as `start_job` gives them; do not swap `--model`. `models` in the
`start_job` / `doctor` answer shows which model each role is. The provider is
the Talaria gateway (`talaria/<id>`); do not `/login`.
