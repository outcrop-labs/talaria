---
name: oh-my-pi
description: Drive Oh My Pi (omp) through a Talaria workbench job as an orchestrator — scoped asks, same session, continue with -c. Use when this agent's chosen harness is Oh My Pi.
---

# Oh My Pi (Talaria)

You are the orchestrator. Oh My Pi (omp) is the pair programmer. Do not
hand-code. Do not one-shot the ticket.

## Path

1. `doctor` if you have not yet this session — read the `guide` and run the `probe`.
2. `start_job` with the ticket, repo, effort, and (for standard/heavy) a plan.
3. Clone into the `workdir`. Work only there, only on the branch it cut.
4. First turn: `jsonRun` (else `run`) in the workdir. One scoped ask. Fill `<task>`.
5. Later turns: `continueJsonRun` / `continueRun` (`-c`, same `--session-dir`).
   Read the JSON events. Steer. Never `--no-session`, never the TUI.
   `--auto-approve` is already on the line so tool calls do not hang.
6. Hash-anchored edits, LSP, subagents, and a browser live *inside* omp —
   do not reimplement them.
7. Git over https:// just works. Never `gh`, never a token in a URL.
8. `git diff`, verify, `finish_job`.

`--model talaria/<id>` is the Talaria gateway. Do not `/login`.
