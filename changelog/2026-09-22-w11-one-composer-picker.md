- **W11 — one composer picker.** `components/chat/ComposerPicker.svelte` absorbs
  `TierPicker` and `EffortPicker` (both deleted) and the seven call sites migrate
  to it. Eleven real differences were enumerated and each became a prop —
  `chipVariant` (`primary` for both deleted chips), `icon` (the tier's ✳ glyph as
  a string), `meter: { total, lit }` (computed at each site: the tier floor is
  `indexOf+1`, the effort chip is 0-lit at `''`), the every-row `MeterBars` on
  primary chips, `autoOption` (effort's "auto / model default" ingress row),
  `label` (the tier chip reads "main" while its row reads "main model"),
  `searchable`/`searchPlaceholder`, `menuClass` (three different min widths:
  44/48/56), `disabled`, and the aria set (effort's trigger carried
  `aria-haspopup`/`aria-expanded`, the other two carried neither — now every
  trigger has them, the one intentional deviation).
  **Parity proved by rendering, not by inspection**: 10 SSR renders of the old
  components (from the pre-sweep commit) against the new one, fed each call
  site's exact props with class-token and attribute order canonicalized — 7/10
  canonical HTML identical, and the 3 tier renders differ only by those two aria
  attributes (deleting the two lines makes them byte-identical). Panel min-widths,
  the search row, the auto/MODEL DEFAULT ingress caption, and the per-row meters
  (1/3, 2/3, 3/3) all matched. This database has no model endpoints, so `tiers`
  is empty and the chips cannot be rendered in the running app; SSR is the
  strongest proof available here, and it cannot speak to open/close, hover or
  popover placement.
