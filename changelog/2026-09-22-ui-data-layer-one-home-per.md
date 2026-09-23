- **UI data layer: one home per helper (part 1).** The sweep's UI wave, done as
  the pieces that are verifiable without a browser:

  * **The three duplicate function bodies the detector was holding open for the
    UI are gone — the allow list is now EMPTY, and `bun run check` reports zero
    clusters in both languages.** `lib/list-nav.ts` owns the
    Arrow/Enter/Tab grammar (EmojiList + MentionList; each menu's exported
    `onKeyDown` is an adapter over a `createListNav` bound to its own runes),
    `lib/outside-click.ts` owns the outside-pointer test (Popover +
    DropdownMenu), `lib/menu-position.ts` owns the caret-anchored placement
    (mention-suggest + slash-commands) — which is also where the two caret
    menus' flip-above rule finally agrees.
  * **`lib/reactive-arg.ts`** — `MaybeGetter<T>` and `resolve()`, declared
    locally in sixteen files (plus two one-off spellings, `resolveModel` and
    `resolveValue`). One type, one resolution.
  * **`toastError(title, e)`** — 93 call sites wrote
    `pushToast({ title, body: errorMessage(e), tone: 'danger' })` by hand; the
    helper is that pairing, and 54 files call it now. Four sites whose body is a
    composed sentence (`\`${label}: ${errorMessage(e)}\``) keep their own call —
    the rule is the pairing, not the tone.

  **Not done in this wave** (and why): `persist.ts`, `statuses.ts`'s
  `STATUS_COLOR`, `format.ts`'s usd/bytes/token/duration spellings, the
  `teams.ts` directory hook, the context-menu/SortHeader helpers and
  `sse.ts`'s `openStream`. Each is a behavioural refactor of a component's
  render path, and this host has **no docker, so no dev stack and no browser** —
  the remaining UI waves (W8–W11: QueryState adoption, the component-pair
  merges, the row-chrome and composer-picker collapses) would be landed
  unexercised and pixel-unverifiable, which is the one thing their wave notes
  say not to do. They are left for a session that can run `bun talaria dev`.

  Verified: `bun run verify` (check + svelte-check 0 errors + 1,183 vitest
  tests).
