- **`doctor` runs the probe instead of printing it, and says when the sandbox
  is stale.** The workbench's self-diagnosis reported
  `harness: Oh My Pi (oh-my-pi), auth through the Talaria gateway` whatever
  the truth was, handed back the probe command as a string, and told the
  agent to run it in its own shell. On the live instance the harness had been
  unable to start for weeks behind that sentence. `bun` resolves through a
  mise shim, and a shim only has a version where a mise config pins one — a
  Rust job pins rust and mold, not bun — so `npx -y @oh-my-pi/pi-coding-agent`
  died before it began. The agent read a green doctor, searched the
  filesystem for an `omp` binary, found none, and built the ticket by hand.
  Proven on the instance: with a real bun ahead of the shims the same probe
  answers `omp/18.4.8`, exit 0. doctor now executes the probe as the AGENT
  USER in a LOGIN shell — the only environment the answer is about, since
  root has a bare PATH and the agent's is built by the profile block
  `prepare_env` writes — and reports the version or the reason, with
  `probeResult` on the wire and a `next` that says report_problem rather than
  "run the probe yourself". A second check covers the case the probe cannot:
  `--version` writes nothing, so it can pass while the harness still has
  nowhere to keep state. The cont-init hook that hands
  `/opt/data/workbench/harness/pi` to the runtime user has been in the render,
  with a test, while the live fleet ran containers rendered before it existed
  — `/etc/cont-init.d` in the running agent holds only Hermes's own three
  scripts, and Docker had created that bind-mount parent as root:root 755, so
  omp could read its mounted config and not write a byte beside it. A render
  change only reaches a container when the agent is re-rendered and ROLLED,
  and nothing forced that or noticed it had not happened. doctor now reports
  the directory as not writable and names the fix. Verified: `bun run check`,
  `cargo fmt --all --check`; the probe and the ownership failure were both
  reproduced on the live instance before the change and the probe's success
  path confirmed there too. The Rust was not compiled locally — CI's
  `fmt + clippy + test (api)` is the proof line.
