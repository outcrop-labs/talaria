- **The coding harness has a bun to run on, wherever it is run.** Oh My Pi is
  a bun program — its installed bin is `#!/usr/bin/env bun`, a symlink to
  `dist/cli.js`, whichever way it is installed. There is no standalone build:
  the official `curl -fsSL https://omp.sh/install | sh` reports "Installed omp
  via bun" and drops the same shebang, and it refuses to run at all without
  bun ("Failed to read bun version"). Meanwhile `bun` on the agent's PATH is
  a **mise shim**, and a shim only resolves where a mise config pins bun. A
  Rust job pins rust and mold, so the shim had no version, and
  `npx -y @oh-my-pi/pi-coding-agent@latest` died before it began — every
  invocation, in every job, since the harness shipped. With a real bun ahead
  of the shims the identical probe answers `omp/18.4.8`, exit 0, so nothing
  else in the chain was wrong. bun is now pinned **globally** for the runtime
  user in two places, because either can be the first to run: the workbench
  cont-init hook that already heals the bind-mount ownership at boot, and
  `prepare_env`, which is what installs mise on a fresh volume. Global rather
  than per-repo on purpose — the repo being worked on has no reason to pin
  the harness's own runtime, and `doctor` probes from the agent's home rather
  than from inside any checkout. Verified against the live instance, which is
  the only place this was ever observable: the probe failing, the installer
  refusing, the installed bin's shebang, and the probe passing once bun
  resolves. The Rust was not compiled locally — CI's
  `fmt + clippy + test (api)` is the proof line, and the render's hook test
  now asserts the pin is filled in and global rather than checking the
  unfilled template.
