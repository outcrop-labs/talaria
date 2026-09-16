---
name: codex
description: Coding harnesses run through the Talaria workbench, not as a raw Codex CLI. Drive jobs with the workbench-driving skill.
---

# Codex (Talaria)

Do not `npx` Codex yourself. If this agent has a workbench grant, the
**workbench-driving** skill is the path: `doctor`, `start_job`, work in the
workdir it gives you, `finish_job` — Talaria cuts the branch and opens the
ticket-linked PR. A human merges.

A harness you cannot drive is `report_gap`, never a reason to silently
hand-code around it. The GitHub half is the **github** skill (plain git over
https; no gh).
