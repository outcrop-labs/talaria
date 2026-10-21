- **Talaria Mobile begins: a scope document, a provisioned toolchain, and a
  building Android app** — [`docs/MOBILE.md`](./docs/MOBILE.md) is the scope of
  record, `mobile/` is the surface. It is a **controller**, not the product on a
  smaller screen, and the line it draws is one sentence: *if the action is
  reversible in five seconds and fits in a sentence it belongs on the phone; if
  it needs a diff, it doesn't.* Agent Studio, Models, MCP governance, Admin,
  the secrets vault, Boards-as-boards and every editing surface are out by
  design rather than by schedule — they need a keyboard, a canvas and an undo
  story.

  **The stack is Kotlin Multiplatform + Compose Multiplatform, and the reason is
  SSE rather than taste.** This app's backbone is three streams —
  `GET /api/me/events`, `GET /api/channels/{id}/events`, and `POST /api/chat`
  tokenizing a reply — which is exactly React Native's weakest surface: no
  built-in `EventSource`, and no real `ReadableStream` from `fetch` on Hermes.
  Ktor streams natively on OkHttp and NSURLSession both. The TypeScript-reuse
  argument that would have favored RN was measured and does not carry: 74 of 112
  files in `ui/src/lib/` touch Svelte or the DOM, leaving roughly a thousand
  portable lines that re-express in Kotlin in about a day. The accepted cost is
  that **iOS cannot be built from Linux at all** — Apple targets are declared in
  `:shared` and compile only on macOS, so Android iterates locally and iOS
  iterates through CI.

  **Two gaps are recorded as api work, not client work.** The session is an
  opaque `sid` cookie with a **7-day absolute TTL and no sliding refresh**
  (`talaria-session/src/lib.rs`), so a phone needs a `tdv_…` device credential —
  sha256-stored like the `tlk_` gateway keys, revocable per device — or every
  account re-logs in weekly and Google sign-in cannot work at all. And APNs/FCM
  credentials belong to the app's publisher rather than the instance, so a
  self-hosted Talaria cannot reach an App Store build's device token: that needs
  a small stateless relay carrying a wake signal and nothing else, which the
  realtime invariant (*an event says what changed, never what it says*) already
  guarantees is all there is to carry.

  **Two modules, because AGP 9 requires it.** `com.android.application` now
  refuses to sit on a KMP module, so `:shared` holds every line of real code via
  `com.android.kotlin.multiplatform.library` and `:androidApp` is an Activity, a
  manifest and a theme. AGP offers `android.builtInKotlin=false` to bypass the
  check and its own message calls that temporary, so this took the structure
  instead of the flag — and the split earns its keep, since anything appearing
  in `:androidApp` is something iOS cannot reach. `:shared` also carries a `jvm`
  target that never ships, so `commonTest` runs on the host instead of needing
  an emulator.

  **The toolchain provisions into the devbox shared layer, not the image.**
  [`mobile/tools/devbox-install.sh`](./mobile/tools/devbox-install.sh) installs
  a JDK, Gradle and the Android SDK into `/work/tools` — rootless, idempotent,
  and with no pinned version numbers (Adoptium's latest-21 redirect, Gradle's
  current-version endpoint, and whatever the SDK reports as newest and stable).
  A JDK plus the Android SDK is multiple gigabytes and most boxes never touch
  mobile, so one download serves every box created afterwards. Three things it
  now knows, each of which cost a failed build: `sdkmanager` is deprecated as of
  cmdline-tools 23 and the replacement CLI **lists** package paths with slashes
  while `--install` still takes semicolons; **API levels ship minor versions
  now**, so API 37 exists only as `android-37.0/.1/.2` and an integer-only
  filter silently picks 36 — which AndroidX Compose 1.12 refuses to compile
  against, failing in `checkDebugAarMetadata` a long way from the cause; and
  `yes | sdkmanager` exits 141 under `pipefail` because the producer takes
  SIGPIPE.

  **Verified.** `./gradlew :shared:jvmTest` — 19 tests pass, covering origin
  normalization and the beacon probe. `./gradlew :androidApp:assembleDebug`
  produces a 14MB debug APK on Temurin 21 / Gradle 9.8.1 / AGP 9.4.1 / Kotlin
  2.4.21 / Compose Multiplatform 1.12.1 / Ktor 3.6.0, `compileSdk` 37.2.
  `bun run check` clean (848 links resolve, `gate.mjs --self-test` covers the
  new `mobile` surface selection). The provisioner was re-run to confirm it is
  idempotent and that it now resolves `android-37.2`.

  **The shared test list earned its place on its first run.** `OriginTest` is
  deliberately the same cases as `desktop/src-tauri/src/beacon.rs`'s test
  module, answer for answer, because the two implementations are independent and
  only a shared list keeps them agreeing — a disagreement gives one instance two
  identities in one person's account list. It immediately caught two places
  where Ktor's parser is lenient and Rust's `url` crate is strict: Ktor reads
  `https://not a url at all` as host `not`, and `URLBuilder` defaults an empty
  authority to `localhost`, so a bare `https://` parsed as a valid
  `https://localhost`. Both are now refused before parsing, since the parsed
  object cannot report what the input was missing.

  Not yet: the account store, anything behind a session, the `iosApp/` Xcode
  project (it cannot be built or verified off macOS, so it lands with the CI job
  that will compile it), and the `mobile` CI jobs.
