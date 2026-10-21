- **Talaria Mobile releases itself, in its own tag namespace** — a `mobile/`
  push to `main` builds the Android APK, mints `mobile-vX.Y.0` and opens a
  GitHub prerelease with the APK attached. Starting at **0.1.0**.

  **It is a deliberate sibling of `desktop-package.yml`, not a second scheme.**
  The question is identical — a surface that produces something a person
  installs needs a version an installer can compare — and that answer is
  already load-bearing in this repo: auto-minor on a surface push, a manual
  door for a major, artifacts-only on a pull request. `mobile-v*` sits beside
  `desktop-v*` for the same reason that namespace exists.

  The one place the two differ is what they count. Desktop's auto-minor reads
  **both** `v*` and `desktop-v*`, because desktop tracks the platform's version
  line. Mobile reads only `mobile-v*`: it is its own artifact on its own
  cadence, cutting versions far faster than the platform cuts its, which is
  also why its first mint is `0.1.0` rather than wherever the platform happens
  to be.

  **The version is injected, never committed**, so
  [`RELEASING.md`](./RELEASING.md)'s rule survives intact — releasing never
  edits a file. `-PmobileVersion=X.Y.Z` arrives from the workflow and is
  stamped for that build only.

  **`versionCode` is derived, not maintained**, because it is the one number
  Android actually enforces: a device *refuses* an APK whose code is not
  greater than the installed one, so a non-monotonic scheme makes an upgrade
  silently impossible to install — and two humans editing one integer is
  exactly how that invariant breaks. `maj*10000 + min*100 + patch`, checked
  across 0.1.0→100, 0.2.0→200, 1.4.7→10407, with a floor of 1 for the
  `0.0.0-sha…` builds that mint nothing.

  **Three traps the logic is written against, two of which bit during
  testing.** A lexical sort puts `0.10.0` below `0.2.0`, so the tenth release
  would mint a version that already existed — the sort is numeric per field. A
  tag pushed with `GITHUB_TOKEN` starts no workflows, so the build and the mint
  have to live in one run rather than hand off to a tag trigger, which is why
  `attach` escalates to `contents: write` instead of delegating. And
  `printf '%s\n' $tags` depends on word-splitting, which bash does and zsh does
  not — so the first version of the logic test was green while the logic was
  wrong; `gh` emits one tag per line and is now piped straight through.

  **Every mobile release is a prerelease, deliberately.** The APK is
  debug-signed with the keystore that ships inside every Android SDK on earth:
  installable with `adb install -r`, and not distributable. `/releases/latest`
  must never resolve to one. A Play-signed build is a second job in that
  workflow once store credentials exist — not an edit to the one that is there.

  **Two things that had to be true before an APK was worth building at all.**
  Android has blocked cleartext HTTP by default since API 28 and this app
  targets 37, so a build pointed at a self-hosted instance on `http://host:port`
  could not reach it — and the symptom is indistinguishable from the server
  being down: the launcher says "could not reach" and nothing mentions that the
  platform refused to dial. A debug-only manifest permits it; release builds
  keep the secure default, because the session cookie rides those requests. And
  "unsigned" is not a thing Android will install: `assembleDebug` signs with
  the debug keystore, which the workflow verifies with `apksigner` alongside
  asserting the cleartext allowance survived the manifest merge. Both checks
  *find* their inputs rather than hardcoding paths — AGP writes more than one
  merged manifest and has moved the spelling between versions, and a check
  pinned to one path becomes a check that silently stops checking.

  **iOS is `continue-on-error` and off the required-checks list** until it has
  ever been green. Android is being tested on real devices first, and a macOS
  job that has never passed must not hold that up; it still runs and still
  reports. The same posture `judge.yml` already holds.

  **Verified.** Both workflows parse; the version-resolution logic checked
  against six tag sets (none → 0.1.0, 0.1.0 → 0.2.0, 0.10.0 → 0.11.0,
  1.0.0 → 1.1.0, malformed tags ignored); the injected version read back out of
  AGP's own output metadata for five inputs; `assembleDebug` green; the merged
  manifest carries `usesCleartextTraffic="true"`; `apksigner` confirms a V2
  debug certificate; `bun run check` clean.
