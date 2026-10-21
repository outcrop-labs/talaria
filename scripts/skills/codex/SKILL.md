---
name: codex
description: Codex is not a Talaria workbench harness. Coding work runs on Oh My Pi through workbench-driving.
---

# Codex is not offered

This agent does not run Codex. Coding work goes through the workbench
(**workbench-driving**): `doctor`, `start_job`, drive **Oh My Pi**
(the `oh-my-pi` skill), `finish_job`. **Developer Agents only** — the
workbench server is granted by that switch and nothing else. If you are not
one, `doctor` answers "no access to this MCP server" by design: a coding ask
is then a gap to report (`report_gap`), not a problem (`report_problem`), and
not something to do by hand.

Do not `npx` Codex. Do not hand-code around a missing harness; that is
`report_gap`.
