# The WORKBENCH image — the Hermes chassis with the coding harnesses and
# Playwright pre-baked, so first runs are instant instead of paying npx
# downloads. Purely an optimization: the stock image works via npx @latest;
# this one just makes it warm from second zero. Versions are unpinned so a
# rebuild picks up upstream; invoke templates also use @latest so a running
# container auto-updates on the next job without a rebuild.
#
# Build:  scripts/build-workbench-image.sh   (tags talaria-workbench:latest)
# Use:    set the dev profile's image to talaria-workbench:latest
#         (PUT /api/workbench {"slug":"dev","image":"talaria-workbench:latest"})
#         then render + roll.
ARG HERMES_IMAGE=nousresearch/hermes-agent:latest
FROM ${HERMES_IMAGE}

# The builtin harnesses, globally installed. Do not pin: auto-update is the
# point. npx @latest on invoke still fetches newer than this layer.
RUN npm install -g \
      opencode-ai \
      @earendil-works/pi-coding-agent \
      @oh-my-pi/pi-coding-agent \
    && npm cache clean --force

COPY workbench-harness-update.sh /usr/local/bin/talaria-harness-update
RUN chmod +x /usr/local/bin/talaria-harness-update

# Playwright + chromium with system deps — UI verification is first-class
# dev work. Browsers land in the image; PLAYWRIGHT_BROWSERS_PATH (set by the
# renderer to the persistent volume) still wins at runtime for updates.
RUN npm install -g playwright \
    && npx playwright install --with-deps chromium \
    && rm -rf /var/lib/apt/lists/*
