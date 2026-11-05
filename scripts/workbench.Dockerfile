# The WORKBENCH image: the Hermes chassis with the dev-env baseline
# pre-baked, so a first job skips the apt and mise downloads. Purely an
# optimization: the stock image works the same way.
#
# Oh My Pi is NOT baked in. Every job runs the version the workbench harness
# channel verified (talaria-workbench-updates; OMP_VERSION in
# talaria-workbench-harnesses is the bootstrap fallback), installed once per
# department volume by the job's provisioning and started through the job's
# own launcher; a global install here would be a second omp nothing runs.
#
# The BASE is not unpinned: it is the same Hermes pin the chassis renders
# (scripts/chassis.template.yml), so a Developer Agent runs the Hermes every
# other agent runs. Bump the two together (docs/ARCHITECTURE.md, "The fleet").
#
# Build:  scripts/build-workbench-image.sh   (tags talaria-workbench:latest)
# Use:    set TALARIA_WORKBENCH_IMAGE=talaria-workbench:latest in the app's
#         env; Developer Agents render on it, then render + roll.
ARG HERMES_IMAGE=nousresearch/hermes-agent:v0.22.4@sha256:7a69201726bc64db2072a1371557137c23312d29705cda2b8ea2a0214a50a4cf
FROM ${HERMES_IMAGE}

# The dev-env baseline prepare_env builds on: a C toolchain and pkg-config
# for native builds, mold for repos that link with it (Talaria does), and mise
# for per-repo toolchains. prepare_env installs any of these on the stock
# image too; baking them in just skips the first-job wait.
# mise is the pinned release binary, checked against its published SHA-256 —
# the same pin a workbench installs at runtime (talaria-workbench-harnesses,
# MISE_VERSION and MISE_SHA256_*); bump them together.
ARG MISE_VERSION=2026.10.19
ARG MISE_SHA256_X64=45fa1d94af35d5c65df7176612d5b3f61f9e406d47e8eb28e5032e2a0b99b563
ARG MISE_SHA256_ARM64=35cbcd510247f9bde9baee667bd0b394a3d78a54078645f40c028a43c4794baa
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential pkg-config mold curl ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && case "$(uname -m)" in x86_64) arch=x64; sum="$MISE_SHA256_X64" ;; aarch64) arch=arm64; sum="$MISE_SHA256_ARM64" ;; *) echo "no pinned mise for $(uname -m)"; exit 1 ;; esac \
    && curl -fsSL -o /usr/local/bin/mise "https://github.com/jdx/mise/releases/download/v${MISE_VERSION}/mise-v${MISE_VERSION}-linux-${arch}-musl" \
    && echo "$sum  /usr/local/bin/mise" | sha256sum -c - \
    && chmod 0755 /usr/local/bin/mise

# No Playwright layer. UI checks go through the agent's own browser tools:
# Hermes ships a pinned Chromium at /opt/hermes/tools and hands it to the
# agent-browser CLI (which Talaria installs onto the agent's volume before
# each start of an agent with the browser and the terminal on; the Developer
# Agent switch opens the terminal) through AGENT_BROWSER_EXECUTABLE_PATH, in
# this same container. A baked Playwright browser was never seen anyway: this image
# inherits PLAYWRIGHT_BROWSERS_PATH=/opt/hermes/tools, so it installed there,
# and the render points PLAYWRIGHT_BROWSERS_PATH at the agent's own state
# volume (/opt/data/workbench/harness/playwright, on hermes-<department>) at
# runtime. That path is where a repo's own e2e suite installs the browser
# revision ITS Playwright pins, on its first run, and where that agent's
# later runs find it again.
