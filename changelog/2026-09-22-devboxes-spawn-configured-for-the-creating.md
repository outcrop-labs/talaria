- **Devboxes spawn configured for the creating shell**: `box new` inherits
  `ANTHROPIC_BASE_URL`/`ANTHROPIC_AUTH_TOKEN` (and `_MODEL` when set) into the box's
  `compose.override.yml` — a shell whose own Claude runs on GLM gets boxes that do too, no
  per-box login. `--env KEY=VALUE` (repeatable) carries any other container env (explicit keys
  beat the inherited trio), and `--setup <cmd>` (repeatable) runs arbitrary provisioning inside
  the fresh box. An explicit `--claude-token` disables the inheritance; the two auth vars never
  ride together.
