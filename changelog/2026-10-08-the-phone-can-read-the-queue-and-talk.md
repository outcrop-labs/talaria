- **Talaria Mobile reads what is waiting on you and holds a conversation** —
  phases 1 and 2 of [`docs/MOBILE.md`](./docs/MOBILE.md): add an instance by
  URL, sign in, see the decision queue, and read and post in channels and DMs
  with the room updating itself off the realtime firehose.

  **`GET /api/home`'s three queues ARE the decision queue** — review gate,
  blocked, triage — so the landing surface is already the one the app exists
  for. Review leads because clearing the gate is the five-second decision the
  whole project is about; blocked is next because an agent is stopped until a
  person moves; triage is last because nothing is waiting on it. Unread trails
  all three and is excluded from "waiting on you", because a message is not a
  decision and a badge that conflates the two stops meaning anything. The
  shapes are transcribed from the api's own structs (`talaria-home`,
  `talaria-channels`) rather than the generated reference, which prints `…` for
  `/api/home` because the response is computed.

  **The port boundary is the load-bearing decision.** Screens ask a `Ports` of
  suspend functions instead of holding an `HttpClient`. The UI layer imports no
  Ktor, so swapping the session cookie for a device credential touches one file
  and no composable — and the headless UI tests stopped being flaky, which is
  what forced it. Driving a real client from a UI test means `waitUntil` races a
  coroutine on another dispatcher and a virtual clock can burn its whole timeout
  before that coroutine is scheduled; it presented exactly as it always does, as
  tests passing and failing on alternate runs. Now behaviour is asserted against
  functions that answer immediately, response shapes in their own decode tests,
  and the wire over a real socket. No test waits on another's scheduler.

  **The UI is driven headlessly** — `runComposeUiTest` composes the real tree
  against Skia's software renderer inside the ordinary `:shared:jvmTest` gate: no
  display, no emulator, no screenshots to eyeball. Three things that cost a
  failed build to learn, now written down: a plain `jvm()` target does not pull
  skiko's native binary the way a Compose Desktop application does, so every UI
  test died in `LibraryLoadException` until `compose.desktop.currentOs` landed on
  the test classpath; CMP 1.12 no longer publishes `iosX64`, so declaring it
  fails resolution for every Compose-dependent source set; and a clickable `Card`
  merges its descendants' semantics, so a tag inside one needs
  `useUnmergedTree`.

  **A UI test caught a third Ktor/Rust divergence.** Ktor preserves host case
  where Rust's `url` crate folds it, so `TALARIA.example.com` and
  `talaria.example.com` were about to become two registry keys for one instance
  — precisely the failure the shared contract with `beacon.rs` exists to
  prevent. (The first two: Ktor reads `https://not a url at all` as host `not`,
  and `URLBuilder` defaults a bare `https://` to `localhost`.) All three are
  refused before parsing now.

  **No ambient cookie jar, deliberately.** Desktop gets account isolation free —
  one cookie jar per webview — while a phone is one process serving every
  account, so a credential is a value attached per request. Two accounts on the
  *same* instance therefore work, which desktop structurally cannot do, and
  tests assert that each one's session rides its own call and that switching
  accounts switches credentials rather than replaying a cached answer.

  **The realtime invariant is asserted rather than trusted.** `UserEvent`
  carries ids and nothing else, so a `channel` event makes the open room
  re-read itself through the ordinary ACL'd route — and a test fails if any
  variant ever grows a content field, which is how realtime would start widening
  reads. The SSE frame parser is a separate pure object because every way it can
  be subtly wrong is invisible from a UI: Talaria sends a `: connected` preamble
  and a `: ping` every 25 seconds, so a parser reading comments as data would
  invent a phantom event every 25 seconds forever, and a network chunk boundary
  does not respect frame boundaries. A lone trailing CR deliberately waits
  rather than guessing, because it cannot be distinguished from a split CRLF.

  **Verified.** `./gradlew :shared:jvmTest` — 101 tests green, of which 34 drive
  real composables headlessly and 9 run against a JDK `HttpServer` over a real
  socket (including one that pins the regression where a stream forgets to opt
  out of the client's short request timeout and dies on a timer while healthy).
  `:androidApp:assembleDebug` green. `bun run check` clean.

  Not yet, and named rather than implied: assistant chat, the review *action*
  (the queue is read-only), fleet control, run watching, cost, plan and
  knowledge reading, the `iosApp/` Xcode project, the `mobile` CI jobs, and
  persistent credential storage — which waits on gap 1 because secure storage
  written against a 7-day session id would be rewritten the moment a device
  token replaces what is stored. A restart signs you out today, which is honest
  about the state of the credential plane.
