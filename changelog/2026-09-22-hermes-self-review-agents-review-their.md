- **Hermes self-review — agents review their own work before the QA judge
  does (#78).** Three moves. First, the plumbing bug that blocked all of it:
  Hermes only discovers skills outside its home via `skills.external_dirs`
  in config.yaml, and Talaria never rendered that key — the `/opt/skills` +
  `/opt/dept-skills` mounts existed in every container but the skill registry
  never scanned them. Rendered configs now carry both roots, so every
  Talaria-managed skill is a first-class enabled Hermes skill (live: `hermes
  skills list` shows them as `local/enabled` fleet-wide). Second,
  `subagent-driven-development` (official Hermes registry, MIT,
  obra/superpowers-derived) is vendored into `scripts/skills/` and seeds to
  every install — with the bundled `requesting-code-review` it gives agents
  fresh-context implementer/reviewer subagents per task. Third, the reflex:
  the talaria-toolkit skill and every rendered soul now teach "before
  `report_outcome`, self-review against the ticket's requirements —
  `requesting-code-review` for code, `subagent-driven-development` for
  multi-task plans" — the QA judge becomes the second line, not the first.
  Bonus: shared-skill seeding upgraded from copy-if-missing to pristine
  tracking (`fleet/skills/.seeds.json`) — copies still byte-identical to what
  was seeded follow canonical updates; admin-edited copies are never
  clobbered (both paths verified live).
