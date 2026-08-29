# Talaria devbox — a disposable, self-contained dev environment for agents
# (Claude Code, opencode) and humans: one container per task, holding a repo
# clone, the CLIs, and a docker-socket pass-through. Docs: docs/DEVBOX.md.
#
#   bun talaria box build
#
# (The wrapper passes --network=host: on hosts whose docker daemon has no
# working upstream DNS, apt/npm inside the build otherwise resolve nothing —
# docs/CONTAINER.md, Troubleshooting.)
#
# A TOOLCHAIN, not the app: the repo arrives as a bind mount at /work/talaria,
# so the image carries only what runs against it — node 22 + bun (the repo's
# toolchain), git + openssh-client (work the clone), the docker CLI + compose
# plugin (the app's fleet code drives the host daemon through the mounted
# socket, exactly like the production container), and the agent CLIs.
#
# glibc (bookworm), not alpine, on purpose: bun's official binary and the
# npm-installed CLIs are glibc-first, and a dev image optimizes for fewest
# surprises, not size — a couple hundred MB once, shared by every box.

FROM docker.io/library/node:22-bookworm-slim

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates curl git openssh-client procps less \
      build-essential cmake pkg-config \
 && rm -rf /var/lib/apt/lists/*

# Docker CLI + compose v2, copied from the upstream CLI image (statically
# linked, so they run fine on bookworm). The socket is mounted in by the box
# compose; `group_add` there grants access.
COPY --from=docker.io/docker:cli /usr/local/bin/docker /usr/local/bin/docker
# The CLI image ships the plugin under libexec; the CLI looks in BOTH
# /usr/local/lib/docker/cli-plugins and /usr/local/libexec/... — land it in
# the former (the path the docs canonically bless).
COPY --from=docker.io/docker:cli /usr/local/libexec/docker/cli-plugins/docker-compose \
     /usr/local/lib/docker/cli-plugins/docker-compose

# Bun, pinned to the repo's packageManager version, from the official debian
# image — same glibc build the repo develops against, and node:22-bookworm-slim
# already carries the libraries it links.
COPY --from=docker.io/oven/bun:1.4.0 /usr/local/bin/bun /usr/local/bin/bunx /usr/local/bin/

# The agent CLIs, pinned. DISABLE_AUTOUPDATER: a self-updating CLI rewrites
# itself into the container's ephemeral writable layer — the version flaps and
# is lost on recreate. Version bumps are image rebuilds (`bun talaria box build`).
ENV DISABLE_AUTOUPDATER=1
RUN npm install -g @anthropic-ai/claude-code@2.1.246 opencode-ai@1.18.23

# Fixed uid 1000 to match the host user who owns the bind-mounted clone — no
# chown dance, files just stay ours. The node image already HAS a uid-1000 user
# (`node`), so adopt it renamed rather than colliding with it. HOME becomes a
# named volume per box (it holds ~/.claude, ~/.config/opencode and the bun
# cache); named volumes initialize from image content on first mount, which is
# how the baked .gitconfig ([safe] directory for the fixed clone path — note
# the section form; `safe.directory` is not a legal dotted KEY) lands in every
# box.
RUN usermod -l dev -d /home/dev -m node && groupmod -n dev node \
 && printf '[safe]\n\tdirectory = /work/talaria\n' > /home/dev/.gitconfig \
 && chown dev:dev /home/dev/.gitconfig \
 && mkdir -p /work/talaria && chown -R dev:dev /work

USER dev
ENV HOME=/home/dev

# Rust toolchain, pinned to the repo's api/rust-toolchain.toml — the port's
# dev runtime (docs/RUST-MIGRATION.md). Installed AS dev, after the usermod:
# ~/.cargo and ~/.rustup then live under HOME, which the box compose mounts as
# a named volume — so the registry cache and toolchain survive box recreation
# (and named volumes initialize from image content on first mount, which is
# how a rebuilt image hands the toolchain to new boxes).
#
# build-essential/cmake/pkg-config above are load-bearing, not dev comfort:
# the api's TLS stack (aws-lc-sys) compiles C, and bookworm-slim ships no
# compiler — without them every `cargo build` dies at the link step.
#
# rustup-init from the distro would also work, but the upstream installer
# pinned to the same 1.97.1 keeps every environment on one toolchain — the
# api/rust-toolchain.toml pin is the source of truth this mirrors (sqlx 0.9's
# MSRV is the floor, not the target). --no-modify-path: PATH is set explicitly
# below so the box's shell profile stays the source of truth for what's on it.
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
 | sh -s -- -y --default-toolchain 1.97.1 --profile minimal --no-modify-path \
 && /home/dev/.cargo/bin/rustup component add rustfmt clippy
ENV PATH=/home/dev/.cargo/bin:${PATH}

WORKDIR /work/talaria
# A shell host, not a process: the compose keeps it alive and everything
# happens through `bun talaria box enter`.
CMD ["sleep", "infinity"]
