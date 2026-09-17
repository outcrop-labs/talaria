---
name: opencode
description: Drive opencode through a Talaria workbench job as an orchestrator — scoped asks, same workdir session. Use when this agent's chosen harness is opencode.
---

# OpenCode (Talaria)

You are the orchestrator. opencode is the pair programmer. Do not
hand-code. Do not one-shot the ticket.

## Path

1. `doctor` if you have not yet this session — read the `guide` and run the `probe`.
2. `start_job` with the ticket, repo, effort, and (for standard/heavy) a plan.
3. Clone into the `workdir`. Work only there, only on the branch it cut.
4. Every turn: `jsonRun` (else `run`) in the workdir. Each run continues the
   project session — follow-ups are another run with the next steer, not a
   new clone. One scoped ask per turn. Fill `<task>`.
5. Read the JSON result. Steer. Repeat.
6. Git over https:// just works. Never `gh`, never a token in a URL.
7. `git diff`, verify, `finish_job`.

Auth is the Talaria gateway (`OPENCODE_CONFIG`). Do not log in.
