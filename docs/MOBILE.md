# Talaria Mobile

**Status: scoped, not built.** This page is the scope of record for the mobile app — what it is,
what it deliberately is not, the two api gaps it opens, and the stack it rides. It becomes the
surface doc as the code lands; until then, nothing in `mobile/` exists.

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
| **Decision queue** | The reason the app exists: pending approvals, the review gate, confirm-sends, blocked agents, mentions. Enough context to decide and two buttons. | `/api/inbox/focus` + `/focus/actions` + `/focus/command`, `/api/brief` (+ `/reply` `/read` `/delegate`), `POST /api/tasks/{id}/review`, `POST /api/chat/chips/approvals/{id}`, `/api/alerts`, `/api/notifications` |
| **Messaging** | Channels, teammate DMs, agent DMs: unread state, read, post, react, read markers, image attachments from camera or roll. | `/api/channels{,/{id}/messages,/.../reactions,/read}`, `/api/conversations`, `/api/dms` |
| **Assistant chat** | The controller's command line — tell an agent to do something, with dictation. The one thing a phone does *better* than a desk. | `POST /api/chat` (SSE) |
| **Ticket triage** | Open a ticket from a notification: read, comment, move status, assign, watch. **Not** the board. | `GET/PUT /api/tasks/{id}`, `/comments`, `/watchers`, `/review` |
| **Home brief** | The landing surface: what is waiting on you. | `GET /api/home`, `GET /api/unreads` |
| **Fleet control** | Which agents are up or blocked; stop, start, restart one; kill a runaway work session. "Controller" taken literally. | `GET /api/fleet/containers`, `POST /api/fleet/agents/{id}/control`, `POST /api/tasks/{id}/work-session/stop` |
| **Run watching** | Read-only stream of what an agent is doing right now. | `GET /api/tasks/{id}/work-session`, `/api/activity` |
| **Cost glance** | What the workforce spent today, per agent. Needs the `view:/observability` grant. | `GET /api/cost` |
| **Plan + knowledge reading** | Read a living plan or a knowledge doc when a decision needs context the queue doesn't carry. **Reading only.** | `GET /api/conversations/{id}/doc`, the knowledge group |

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

`mobile/` sits at the root beside [`desktop/`](../desktop), and registers the same way a new surface
always does here:

- Root scripts: `bun run mobile` (Android dev) and `bun run mobile:check` (ktlint + compile +
  unit tests) — surface-gated like `desktop:check`, **not** part of the local default.
- [`scripts/gate.mjs`](../scripts/gate.mjs): a `mobile` surface branch, so a `mobile/`-only diff
  compiles and tests only that. Mirrors how `desktopCargo` / `desktopTypecheck` select.
- The devbox image grows a JDK and the Android SDK ([`docs/DEVBOX.md`](./DEVBOX.md)).
- CI: a `mobile` job in `ci.yml`, and a `mobile-package.yml` that builds the Android artifact on
  `ubuntu-latest` and the iOS one on `macos-latest`, attached to releases as the desktop installers
  are.

## Gates

`bun run mobile:check` is the surface gate, and the CI `mobile` job runs the same list — the
per-surface pattern `api:check` and `desktop:check` already establish. `bun run gate` stays the local
pre-push: `check` always, then only the surfaces the diff touched. Do not run a workspace Gradle
build for the same reason you do not run a workspace cargo — it pins the machine.
