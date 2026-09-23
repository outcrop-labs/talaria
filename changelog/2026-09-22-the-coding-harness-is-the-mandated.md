- **The coding harness is the mandated path for code work — named, keyed,
  unblocked, and skilled.** Four pieces, one contract: the agent that was
  handed a harness drives it instead of hand-coding. The dispatch brief now
  NAMES the agent's selected harness (the platform knows workbench_harness —
  no "whichever is configured" hedging) and points at `doctor` for its guide;
  hand-editing files is reserved for trivial one-line fixes, and a harness
  the agent cannot drive is a report_gap, never a reason to silently
  hand-code. Claude Code's auth finds the org's model access wherever it
  lives, through a three-step lookup that never needs an "anthropic" endpoint
  to be configured (the platform's endpoint rows are OpenAI-shaped by
  construction): a REFERENCE TABLE of providers with known fixed
  Anthropic-protocol surfaces (anthropic native; OpenRouter's first-party
  /api/anthropic; DeepSeek's /anthropic) answers by slug with zero network;
  otherwise a one-time probe of {base}/v1/messages — the row's own base URL,
  or the origin of the provider's native base — verifies the surface and
  CACHES the verdict on the endpoint row (llm_endpoints.anthropic_base), so
  the network half runs at most once per endpoint for the life of the
  install; the hit becomes ANTHROPIC_BASE_URL with the same key riding
  ANTHROPIC_AUTH_TOKEN — no OAuth login — and the no-key-anywhere case now
  WARNS at render instead of arming a harness that fails silently
  (the silence that hid harness non-use across the fleet: zero Claude Code
  sessions ever, zero workbench jobs ever, Hermes hand-coding everything).
  Every armed harness also runs unattended-clean and skilled: onboarding
  cleared and permissions bypassed via read-only policy files mounted at its
  CLAUDE_CONFIG_DIR paths, the fleet skills HOST DIRECTORY bind-mounted
  directly into Claude Code's and Codex's skill directories (a symlink to
  the container-only /opt/skills dangles on the host, and the docker daemon
  answers a dangling bind source with mkdir "file exists" — the first roll
  after the initial merge 500'd every agent up; the direct mount is the
  fix), and AGENTS.md/CLAUDE.md pointers in the workspace telling every
  harness where the skills live.
