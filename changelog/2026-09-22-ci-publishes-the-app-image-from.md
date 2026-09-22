- **CI publishes the app image from main.** Every push to main that touches
  running bits now builds the full app image and publishes it as
  `ghcr.io/outcrop-labs/talaria:sha-<sha12>` (immutable, first) and `:main`
  (pointer, second) — trunk images, beside the release channels a human
  cuts. The api stage is digest-pinned before each build: an api-touching
  push waits for that same commit's api package (built in parallel by
  api-package's own run), anything else pins the digest
  `talaria-api:main` names — a trunk image never mixes a new UI with an
  unbuilt api. No behavior change for any running instance: the feed is the
  groundwork the in-app rolling updater consumes; `TALARIA_INSTALL=image`
  now ships in the image's env as the signal that updater will key on
  (absent in checkout installs, where it stays dormant).
