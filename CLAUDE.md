@AGENTS.md

Claude-specific: repo skills live in `.claude/skills/` (dir name = the slash command;
opencode discovers the same files natively). The tracked `.claude/settings.json` carries
only the Stop hook; personal permissions belong in `.claude/settings.local.json`, which is
untracked.
