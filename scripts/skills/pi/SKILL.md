---
name: pi
description: Drive the Pi coding harness through a Talaria workbench job as an orchestrator — scoped asks, same session, continue with -c. Use when this agent's chosen harness is Pi.
---

# Pi (Talaria)

You are the orchestrator. Pi is the pair programmer. Do not hand-code. Do
not one-shot the ticket.

## Path

1. `doctor` if you have not yet this session — read the `guide` and run the `probe`.
2. `start_job` with the ticket, repo, effort, and (for standard/heavy) a plan.
3. Clone into the `workdir`. Work only there, only on the branch it cut.
4. First turn: `jsonRun` (else `run`) in the workdir. One scoped ask. Fill `<task>`.
5. Later turns: `continueJsonRun` / `continueRun` (`-c`, same `--session-dir`).
   Read `message_end` / `agent_end`. Steer. Never `--no-session`, never the TUI.
6. Git over https:// just works. Never `gh`, never a token in a URL.
7. `git diff`, verify, `finish_job`.

Auth is the Talaria gateway (`--provider talaria`). Do not `/login`.
