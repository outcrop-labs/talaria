- **Workbench coding harnesses are opencode, Pi, and Oh My Pi.** Claude Code
  and Codex are gone from the builtin registry and the seeded `dev` profile
  (a migration strips them from existing profiles and clears per-agent picks).
  All three authenticate through Talaria's gateway (`OPENAI_BASE_URL` /
  `OPENAI_API_KEY` / `LLM_WORKBENCH_API_KEY`) — Pi and Oh My Pi get a rendered
  `models.json` `talaria` provider. Hermes is the orchestrator, not a script:
  first turn is `jsonRun`, later turns are `continueJsonRun` (`-c`) against a
  per-job `--session-dir` (opencode continues by running again in the workdir).
  Print/json mode, no TUI; `--auto-approve` / `-a` so tool and trust prompts
  cannot hang. Invoke templates use `npx @latest` so CLIs auto-update; the
  workbench image preinstalls them and `talaria-harness-update` refreshes
  globals. Git in the sandbox uses `/usr/local/bin/git-credential-talaria`
  via `/etc/gitconfig` (`GIT_CONFIG_SYSTEM` set) — clone URLs carry no token.
  Skills teach driving, not forbidding the CLI. Dispatch forbids hand-coding
  and one-shotting. Verified: harness unit tests including `fill_harness_cmd`;
  `talaria_provider_models_json`; gitconfig pin; `bun run check`; live
  `--version` on opencode 1.18.31, pi 0.85.1, omp 18.2.4.

### Fixed
