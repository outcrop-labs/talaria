# Talaria Mobile

**Status: every surface in the scope below is built.** `mobile/` is a working Android app with five
surfaces — the decision queue (with the review gate), comms, the assistant, the fleet, and reading
— over 243 tests, of which 80 drive the real composables headlessly, 9 run over a real socket and
7 run against a live instance when pointed at one.

**Not yet**, and named rather than implied: the `iosApp/` Xcode project (the shared framework
compiles and links in CI, but nothing consumes it yet), persistent credential storage (**a restart
signs you out** — it waits on gap 1 for the reason given there), push notifications (gap 2),
agent DMs as a distinct surface from channels, and a `mobile-package.yml`.

This page is the scope of record: what the app is, what it deliberately is not, the two api gaps it
opens, and the stack it rides.

The mobile app is a **controller**, not the product on a smaller screen. Talaria's own gap
analysis named the shape years before the app
([`docs/history/PRODUCT-GAPS-2026-07-31.md`](./history/PRODUCT-GAPS-2026-07-31.md), G5): what the
platform asks a manager to do many times a day is *look at a thing an agent did and say yes or no*
— clear a QA gate, approve a confirm-send, unblock an agent, ratify a drafted skill. Five-second
decisions on a paragraph of text: the most phone-shaped interaction pattern in software. Without a
phone, agent throughput is capped by its manager's desk time, which contradicts the whole value
proposition of staff that work around the clock.

So the scope line is a sentence, and it is the one to argue with when a feature request arrives:

> **If the action is reversible in five seconds and fits in a sentence, it belongs on the phone.
> If it needs a diff, it doesn't.**

## What it inherits from the desktop shell

The connect-out model is [`docs/DESKTOP.md`](./DESKTOP.md)'s, unchanged: an instance **is its
origin**. Adding one normalizes the URL (bare hosts become `https://`, only http(s), credentials
and paths stripped), then probes the beacon — `GET /api/well-known/talaria-instance` →
`{instance, companyName}`. A valid beacon uuid is what makes a URL "a Talaria instance", and it is
the dedupe key; the label is `companyName` falling back to host(:port). The shell holds URLs, never
instance credentials.

Where mobile **differs, and is stronger**: desktop gets account isolation for free, because each
instance webview has its own `data_directory` and therefore its own cookie jar. A native client has
one process and one HTTP stack, so isolation has to be explicit — and explicit is better. Every
account is a record of `(instanceId, origin, userId, credential)` in the platform keystore, and
every request carries its own account's credential as a header. There is no ambient cookie jar to
leak across instances, which means two accounts on the *same* instance work as naturally as two
instances do. Desktop cannot do that.

## The stack: Kotlin Multiplatform + Compose Multiplatform

| Layer | Choice |
| :--- | :--- |
| UI | Compose Multiplatform (iOS stable since 1.8.0, May 2025; Metal-backed, 120Hz, iOS-native text selection and scroll) |
| Wire | Ktor client — first-class SSE, riding OkHttp on Android and NSURLSession on iOS |
| Models | `kotlinx.serialization` against the house envelope ([`docs/API-CONVENTIONS.md`](./API-CONVENTIONS.md)) |
| Concurrency | coroutines + `Flow` |
| Credentials | Keychain / Keystore via multiplatform settings |
| Design | [`docs/design/mercury-spec.md`](./design/mercury-spec.md) → a Kotlin token module; the spec carries exact hex and a measured type scale |

**Why not React Native, which would have matched the repo's TypeScript.** Because this app's
backbone is SSE, and SSE is React Native's weakest surface: no built-in `EventSource`, and `fetch`
streaming on Hermes gives no real `ReadableStream`, so the three streams this app lives on —
`GET /api/me/events` (the per-user firehose, built to make runs multi-device),
`GET /api/channels/{id}/events`, and `POST /api/chat` tokenizing a reply — land on
`react-native-sse` or XHR progress hacks. Ktor streams natively on both platforms, and
coroutines/`Flow` fit "three concurrent event streams fanning into UI state" far better than
emitter patterns.

The TypeScript-reuse argument that would have favored RN was measured and does not carry: 74 of 112
files in `ui/src/lib/` touch Svelte or the DOM. The portable, mobile-relevant remainder is roughly a
thousand lines — [`chips.ts`](../ui/src/lib/chips.ts) (the approval-chip logic),
[`daily-brief-types.ts`](../ui/src/lib/daily-brief-types.ts),
[`inbox-focus-types.ts`](../ui/src/lib/inbox-focus-types.ts),
[`sse-parse.ts`](../ui/src/lib/sse-parse.ts), [`agenda.ts`](../ui/src/lib/agenda.ts),
[`dates.ts`](../ui/src/lib/dates.ts) — which re-expresses in Kotlin in about a day. Reuse is close
to a wash, so it does not decide the question.

**The cost, stated plainly:** iOS compile/link/sign requires Xcode on macOS and KMP has no
first-party cloud build service. Android iterates locally; **iOS iterates through CI**
(`macos-latest`, already in use by
[`desktop-package.yml`](../.github/workflows/desktop-package.yml)) and TestFlight. That is a
minutes-long loop for iOS-specific UI work, and it is the accepted price of this choice. Develop
against Android, verify on iOS.

## What ships

Every surface below is served by routes that **already exist** — this is overwhelmingly a client
build, which is why the scope is this wide.

| Surface | What the phone does | Routes |
| :--- | :--- | :--- |
| **Decision queue** | The reason the app exists. `/api/home`'s three queues lead; a queue item opens its ticket, which carries the description, the agent's own outcome and the comments, and the gate offers Approve / Send back. **Approving is a mark, not a move** — see below. | `/api/inbox/focus` + `/focus/actions` + `/focus/command`, `/api/brief` (+ `/reply` `/read` `/delegate`), `POST /api/tasks/{id}/review`, `POST /api/chat/chips/approvals/{id}`, `/api/alerts`, `/api/notifications` |
| **Messaging** | Channels, teammate DMs, agent DMs: unread state, read, post, react, read markers, image attachments from camera or roll. | `/api/channels{,/{id}/messages,/.../reactions,/read}`, `/api/conversations`, `/api/dms` |
| **Assistant chat** | The controller's command line — tell an agent to do something, with dictation. The one thing a phone does *better* than a desk. | `POST /api/chat` (SSE) |
| **Ticket triage** | Open a ticket from a notification: read, comment, move status, assign, watch. **Not** the board. | `GET/PUT /api/tasks/{id}`, `/comments`, `/watchers`, `/review` |
| **Home brief** | The landing surface: what is waiting on you. | `GET /api/home`, `GET /api/unreads` |
| **Fleet control** | Which agents are up or blocked; stop, start, restart one; kill a runaway work session. "Controller" taken literally. | `GET /api/fleet/containers`, `POST /api/fleet/agents/{id}/control`, `POST /api/tasks/{id}/work-session/stop` |
| **Run watching** | The live session on a ticket: model, phase, turn, and the tail of the agent's last reply — plus the switch that ends it. | `GET /api/tasks/{id}/work-session`, `/api/activity` |
| **Cost glance** | Today's spend, one line on the fleet tab. Needs the `view:/observability` grant, and a 403 reads as "not shown for this account" rather than as an error — most members do not hold that view. | `GET /api/cost` |
| **Plan + knowledge reading** | Search the knowledgebase, or open a plan's living document. **Reading only.** A markdown *subset* renders it — headings, lists, quotes, inline marks — and anything it does not know degrades to its own source line rather than vanishing. | `GET /api/conversations/{id}/doc`, the knowledge group |

### How it is tested

Three layers, and the split exists because an earlier version collapsed them and was flaky:

| Layer | What it answers | How |
| :--- | :--- | :--- |
| **Headless UI** | behaviour — does a bad URL show a reason, does an expired session drop the account, does each account's own credential ride its own request | `runComposeUiTest` (the **v2** API, whose `StandardTestDispatcher` default queues coroutines the way production does) composes the real tree against Skia's software renderer. No display, no emulator, no screenshots. |
| **Wire shape** | does the api's JSON decode into what the screens read — camelCase fields, admin-only nulls, absent-vs-null, unknown fields tolerated | plain decode tests against payloads transcribed from the api's structs |
| **Real socket** | does the client speak HTTP — url building, header writing, multi-attribute `Set-Cookie` parsing, a chunked stream read as it arrives | a JDK `HttpServer` on an ephemeral port, driven with Ktor's CIO engine |

**A fourth layer, opt-in: a real instance.** `LiveInstanceTest` runs the real client against a real
Talaria, because the three layers above can all agree with each other and still be wrong together —
the `/api/home` models were transcribed from the api's Rust structs by hand, since the generated
reference prints `…` for that route, and only a live api can confirm the transcription. It is gated
on the environment and **skips** (reported as skipped, not passed) when unset, because a test that
needs a particular instance to exist cannot be a test everyone runs:

```sh
TALARIA_LIVE_ORIGIN=http://your-instance:6302 ./gradlew :shared:jvmTest
# plus, for the routes behind a session:
TALARIA_LIVE_USERNAME=… TALARIA_LIVE_PASSWORD=… ./gradlew :shared:jvmTest --rerun-tasks
```

Provision the credentials with
[`mobile/tools/live-test-account.sh`](../mobile/tools/live-test-account.sh) rather than by hand. It
creates a dedicated least-privilege account (`--admin` promotes it, which is the only way
`/api/home`'s admin-only `alerts` and `costToday` get exercised — they are null for a member by
design) and writes the credentials to a 0600 env file. Neither secret is ever printed: your admin
password is read from a prompt and never stored, and the test account's password is *generated*
rather than asked for, so it reaches no shell history, no terminal transcript and no agent's
context. The file is the only copy; `set -a; . "$file"; set +a` before the run.

`TALARIA_LIVE_ORIGIN` is a tracked Gradle input so exporting it re-runs the task; the credentials
are passed through **untracked**, because an input becomes part of the build cache key and a
password does not belong in build metadata. Changing only the credentials therefore needs
`--rerun-tasks`, which is the right way round.

It is **read-only by design**. The instance it points at is somebody's real workspace with a live
agent fleet in it: nothing posts a message, moves a ticket or clears a gate, because an integration
test must not appear in a colleague's unread count. Assertions are about shape and never content,
for the same reason — a test that printed a real channel's name would leak a workspace into a CI
log.

**The UI tests fake the ports, and that is the point.** Driving a real client from a UI test means
`waitUntil` is racing a coroutine on another dispatcher, and a virtual test clock can burn its whole
timeout before that coroutine is ever scheduled — which shows up exactly as it always does, as tests
that pass and fail on alternate runs for no visible reason. So screens depend on a `Ports` of
suspend functions ([`net/Ports.kt`](../mobile/shared/src/commonMain/kotlin/app/talaria/mobile/net/Ports.kt));
UI tests hand in functions that answer immediately, and the wire is answered where it lives. No test
waits on another's scheduler. The second payoff is that the UI layer imports no Ktor at all, so
swapping the session cookie for a device credential touches one file and no composable.

Tests address **test tags, not visible copy**, so rewording a label is a copy change rather than a
test failure. The one exception worth knowing: a clickable `Card` merges its descendants' semantics,
so a tag inside one needs `useUnmergedTree = true` to be addressable.

### Three rules this app learned from the api, and keeps

1. **Approving a ticket is a MARK, not a move.** The api sets `approvedAt` and leaves the column
   alone, because a reviewed ticket usually still has a merge or a release ahead of it; forcing it
   to `done` would claim the work had shipped when it had not. Rejection is the move. So the buttons
   read "Approve" and "Send back", nothing on the phone can reach `done`, and the screen says the
   rule out loud underneath them. That is the repo's own law — agents create and triage, a human
   signs off, a human moves it — arriving on a phone.
2. **An event says what changed, never what it says.** Every `UserEvent` carries ids only, so a
   `channel` event makes the open room re-read through the ordinary ACL'd route. A test asserts the
   inverse and fails if any variant ever grows a content field, which is how realtime would start
   widening reads.
3. **A KB search snippet is pre-escaped HTML, not text.** `ts_headline` does not escape what it
   excerpts, so the api escapes the whole snippet and swaps its own sentinels for `<b>` — the XSS
   boundary is server-side on purpose. A Compose client that drew that string raw would show a
   reader literal tags; `text/Rich.kt` undoes exactly what the api did, and nothing else in the
   string is treated as markup.

### Sequence

The table above is one scope, not one push — it is a wide v1, and the honest reason it can be is
that almost none of it needs api work. Build it in this order, because each phase is independently
dogfoodable and each one de-risks the next:

1. **Shell and identity.** The instance registry and beacon probe, the account store, the typed Ktor
   client, the Mercury token module. Ends when two accounts on two instances both answer `/api/me`.
   Needs the device credential (gap 1) first, or it gets built twice.
2. **Talk.** Messaging and assistant chat, on `GET /api/me/events` and `POST /api/chat`. The SSE
   plumbing is the riskiest engineering in the app, so it goes early and carries two surfaces.
3. **Decide.** The decision queue and ticket triage. This is the surface that justifies the project;
   it is third only because it is worthless without a notification to open it from — which is why
   the push relay (gap 2) lands alongside it.
4. **Watch.** Fleet control, run watching, cost glance. Small additions on plumbing phases 1–3
   already built.
5. **Read.** Plan and knowledge reading. Last and deliberately so: Mercury's document rendering in
   Compose is the single largest UI cost in the scope, and nothing before it depends on it.

### Out by design, not by schedule

Agent Studio (skills and workflow authoring) · agent design and hiring · Models and providers ·
MCP governance · every Admin panel · the secrets vault · Workbench repo and flow configuration ·
Boards *as boards* (kanban, grouped list, Gantt, saved views) · Knowledge and Artifact **editing** ·
the app platform (`/x/:app`) · Observability dashboards.

One reason covers all of them: these are configuration and composition surfaces. They need a
keyboard, a canvas, and an undo story. The phone's job is to decide, to talk, and to watch.

## The two api gaps

Everything else the app needs is already served. These two are not, and both are api work, not
client work.

### 1. A device credential

Today a session is an opaque `sid` in an HttpOnly cookie, `talaria_session`, resolved against Redis
— and its TTL is **7 days absolute with no sliding refresh**
([`api/crates/talaria-session/src/lib.rs`](../api/crates/talaria-session/src/lib.rs): `const
SESSION_TTL_SECONDS`, and `update_session_user` writes with `KEEPTTL`). A native client *can* log in
with a password and replay that sid as a `Cookie` header with no api change at all — but every
account would re-log in weekly, and Google sign-in would not work at all, because the OAuth callback
sets a browser cookie and has no custom-scheme redirect to hand back to an app. A controller that
signs you out while you are away defeats its own purpose.

So: a `tdv_…` **per-device credential**, sha256-stored exactly as the `tlk_` gateway keys are, listed
and revocable per device in Settings → Devices, exchanged for sessions. It also unlocks in-app
Google sign-in through `ASWebAuthenticationSession` / Custom Tabs with a custom-scheme redirect.
One focused api work item, and it is what makes multi-account and notification deep-links usable
rather than merely possible.

### 2. A push relay

The existing push plane is **Web Push** — RFC 8030/8291/8292, a VAPID keypair born once and sealed
in the database, `push_subscriptions` unique on the push service's endpoint, aes128gcm payloads
([`api/crates/talaria-push/src/lib.rs`](../api/crates/talaria-push/src/lib.rs)). A native app cannot
use it, and the reason a native app cannot simply hold its own credentials is structural: **APNs and
FCM credentials belong to the app's publisher, not the instance.** A self-hosted Talaria has no way
to reach an App Store build's device token without our signing key. Shipping per-instance
credentials would mean every self-hoster publishing their own build, which is not a product.

The answer has a precedent — Matrix's Sygnal. A **small stateless relay we operate**: the instance
posts `{device token, opaque wake blob}`, the relay fans out to APNs and FCM. It sees no content,
because Talaria's realtime invariant already guarantees there is none to see — *an event says what
changed, never what it says* ([`docs/ARCHITECTURE.md`](./ARCHITECTURE.md), Realtime). The banner text
is fetched on-device by an iOS Notification Service Extension (and the Android FCM service handler)
using that device's own credential, through the ordinary ACL'd route. The relay's privacy posture is
therefore the same one Web Push already has, since Apple, Google and Mozilla relay every one of
those today.

The relay URL is a config field defaulting to Talaria's, so an operator who wants no third party can
point at their own.

**Two things this costs that are not code:** a new deployable we run, and store credentials — an
Apple developer account, a Play console, bundle identifiers, signing keys as repo secrets, and
review lead time. Start those early; they gate the first TestFlight build, not the first commit.

## Layout

`mobile/` sits at the root beside [`desktop/`](../desktop), as its own Gradle build with its own
gates — the same per-surface shape `api/` and `desktop/` already have.

```
mobile/
├── settings.gradle.kts        two modules, and why
├── build.gradle.kts           every plugin version, resolved once
├── gradle/libs.versions.toml  the version catalog
├── gradlew                    committed: the build pins its own Gradle
├── shared/                    EVERY line of real code
│   └── src/{commonMain,commonTest,iosMain}
├── androidApp/                an Activity, a manifest, a theme. Nothing else.
└── tools/devbox-install.sh    the toolchain provisioner
```

**Two modules, because AGP 9 requires it.** Since 9.0 the `com.android.application` plugin refuses
to sit on a Kotlin Multiplatform module — it fails the build outright. The supported shape is a KMP
library (`:shared`, using `com.android.kotlin.multiplatform.library`) plus a thin Android
application that consumes it. AGP offers `android.builtInKotlin=false` to bypass the check and its
own message calls that temporary, so this build took the structure instead of the flag. The split
earns its keep anyway: anything that appears in `:androidApp` is something iOS cannot reach, which
makes it a bug by construction.

`:shared` also carries a **`jvm` target that never ships**. It exists so `commonTest` runs on the
host — `./gradlew :shared:jvmTest`. Without it, asserting that a URL normalizes would need an
emulator, and the iOS test targets cannot run on Linux at all.

How it registers:

- Root scripts: `bun run mobile` (build and install the debug APK on a connected device) and
  `bun run mobile:check` (`:shared:jvmTest` + `:androidApp:assembleDebug`) — surface-gated like
  `desktop:check`, **not** part of the local default.
- [`scripts/gate.mjs`](../scripts/gate.mjs): a `mobile` surface branch, so a `mobile/`-only diff
  runs only that. Mirrors how `desktopCargo` / `desktopTypecheck` select, and its `--self-test`
  covers the selection.
- The toolchain is **not** in the devbox image — see below.
- CI: **`test + assemble (mobile)`** on `ubuntu-latest` runs the surface gate, and
  **`compile + link (mobile ios)`** on `macos-latest` is the only place the Apple targets are ever
  built — Kotlin/Native needs Xcode's linker, so until that job runs they have never been compiled
  at all. It compiles both targets and then *links* the framework, because a klib can compile and
  still fail to link, and linking is what an `iosApp/` Xcode project will depend on. Both are
  selected by the `changes` job and listed in
  [`docs/BRANCHES.md`](./BRANCHES.md)'s protection recipe — **re-run that recipe for them to become
  required.**   **`compile + link (mobile ios)` is `continue-on-error` and not a required check** — Android is
  being tested on real devices first, and a macOS job that has never been green must not hold that
  up. It still runs and still reports; it becomes blocking the day it passes.
- [`mobile-package.yml`](../.github/workflows/mobile-package.yml) builds the **debug-signed** APK
  and attaches it to the run. *Debug-signed, not unsigned* — Android will not install an unsigned
  APK at all, and `assembleDebug` signs with the SDK's auto-generated debug keystore, which is what
  makes a sideloadable build possible without owning a release key. It is emphatically not
  distributable: that keystore ships with every Android SDK. A release job arrives with the store
  credentials, as a second job rather than an edit to that one.

## The toolchain

[`mobile/tools/devbox-install.sh`](../mobile/tools/devbox-install.sh) provisions a JDK, Gradle and
the Android SDK into a devbox's **shared** `/work/tools` layer — the one every box mounts at the same
path with `/work/tools/bin` already first on `PATH`:

```sh
bun talaria box new mobile --branch <your-branch>
bun talaria box install mobile 'bash /work/talaria/mobile/tools/devbox-install.sh'
bun talaria box enter mobile bash -lc 'cd /work/talaria/mobile && ./gradlew :shared:jvmTest'
```

It is **not baked into `talaria-devbox:latest` on purpose**: a JDK plus the Android SDK is multiple
gigabytes, and most boxes never touch mobile. Installing into the shared layer means one download
serves every box created afterwards. It is rootless (the box runs as an unprivileged `dev`, and
nothing here needs apt) and idempotent.

### Testing on a real phone

Two things have to be true, and the second one is not obvious:

1. **The phone has to reach the instance.** A self-hosted instance on a LAN (`http://host:6302`)
   means the phone must be on that network or the same tailnet.
2. **The build has to be allowed to dial plaintext.** Android has blocked cleartext HTTP by default
   since API 28 and this app targets 37, so a debug build carries
   [`androidApp/src/debug/AndroidManifest.xml`](../mobile/androidApp/src/debug/AndroidManifest.xml)
   to permit it. Release builds keep the secure default, deliberately — a shipped app should refuse
   plaintext, because the session cookie rides those requests and `Secure` is the flag the api sets
   on it in production. Without that debug manifest the symptom is indistinguishable from the server
   being down: the launcher says "could not reach" and nothing mentions that the platform refused to
   dial.

**Known friction while testing:** credentials are in memory until gap 1 lands, so **every app
restart signs you out**. On a device that is one re-login per launch.

### The box builds, the host installs

A devbox has no USB, so `adb` inside it cannot see a phone — the same split
[`docs/DESKTOP.md`](./DESKTOP.md) describes for the GUI, for the same reason. The APK the box
builds is host-visible, so the host only needs `adb`:

```sh
# host, once (Fedora)
sudo dnf install android-tools

# in the box: build
bun talaria box enter mobile bash -lc 'cd /work/talaria/mobile && ./gradlew :androidApp:assembleDebug'

# on the host: install the artifact the box just built
adb install -r ~/Development/devboxes/<box>/talaria/mobile/androidApp/build/outputs/apk/debug/androidApp-debug.apk
```

`bun run mobile` (`:androidApp:installDebug`) is the one-step version of that pair, and it only
works where `adb` can see the device — on a host that has the toolchain, or in a box pointed at an
`adb connect` target over TCP. From a devbox with a USB-attached phone, use the two steps above.

It pins no version numbers — the JDK comes from Adoptium's "latest 21" redirect, Gradle from its own
current-version endpoint, and the platform from whatever the SDK reports as newest and stable. That
follows the house rule about catalogs: no maintained lists, fetch live. Three things it knows that
are not obvious, each of which cost a failed build to learn:

- **`sdkmanager` is deprecated as of cmdline-tools 23**, and the replacement `android` CLI prints
  package paths with *slashes* (`platforms/android-36`) while `--install` still takes the classic
  *semicolon* coordinates (`platforms;android-36`). Parse one spelling, install the other.
- **API levels now ship minor versions.** API 37 exists only as `android-37.0/.1/.2` — there is no
  plain `android-37` — so an integer-only filter silently selects 36, and AndroidX Compose 1.12
  refuses to compile against anything below 37. The failure surfaces in `checkDebugAarMetadata`, a
  long way from the cause.
- **`yes | sdkmanager` exits 141 under `set -o pipefail`**: the consumer finishes first, `yes` takes
  SIGPIPE, and pipefail faithfully reports it. The prompts still need answering, so judge those
  calls by the consumer's status.

Verified on this toolchain: Temurin 21, Gradle 9.8.1, AGP 9.4.1, Kotlin 2.4.21, Compose
Multiplatform 1.12.1, Ktor 3.6.0, `compileSdk` 37.2, `minSdk` 26.

**One catalog trap worth keeping:** Compose Multiplatform's `material3` artifact is *not* on the
toolkit's version line — it tracks androidx's own material3 numbering and lags by several minors
(1.9.0 against a 1.12.1 toolkit). Asking for `material3:1.12.1` resolves nothing at all. AGP is
likewise **not on Maven Central** — the Central copy stops at 2.3.0, a 2017 artifact, so it comes
from Google's maven or not at all.

## Gates

`bun run mobile:check` is the surface gate, and the CI `mobile` job will run the same list — the
per-surface pattern `api:check` and `desktop:check` already establish. `bun run gate` stays the local
pre-push: `check` always, then only the surfaces the diff touched.

What a Linux box can prove: `:shared:jvmTest` (the shared logic) and `:androidApp:assembleDebug`
(that the whole Android chain assembles). What it cannot: anything Apple. `iosArm64`,
`iosSimulatorArm64` and `iosX64` are declared in `:shared` and compile only on macOS —
`kotlin.native.ignoreDisabledTargets` in `gradle.properties` is what lets a Linux box *configure*
this build rather than fail it. That is why `iosApp/` is not in the repo yet: an Xcode project that
cannot be built or verified here would be a file nobody can check, so it lands with the macOS CI job
that will compile it.
