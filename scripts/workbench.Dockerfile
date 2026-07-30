# The WORKBENCH image — the Hermes chassis with the coding harnesses and
# Playwright pre-baked, so first runs are instant instead of paying npx
# downloads. Purely an optimization: the stock image works via npx + the
# persistent cache; this one just makes it warm from second zero.
#
# Build:  scripts/build-workbench-image.sh   (tags talaria-workbench:latest)
# Use:    set the dev profile's image to talaria-workbench:latest
#         (PUT /api/workbench {"slug":"dev","image":"talaria-workbench:latest"})
#         then render + roll.
ARG HERMES_IMAGE=nousresearch/hermes-agent:latest
FROM ${HERMES_IMAGE}

# The builtin harnesses, globally installed — versions float with the image
# build; agents can still npx a newer one explicitly if a job demands it.
RUN npm install -g \
      @anthropic-ai/claude-code \
      @openai/codex \
      opencode-ai \
      @oh-my-pi/pi-coding-agent \
    && npm cache clean --force

# Playwright + chromium with system deps — UI verification is first-class
# dev work. Browsers land in the image; PLAYWRIGHT_BROWSERS_PATH (set by the
# renderer to the persistent volume) still wins at runtime for updates.
RUN npm install -g playwright \
    && npx playwright install --with-deps chromium \
    && rm -rf /var/lib/apt/lists/*
