// WHERE YOU WERE, remembered by the view itself.
//
// Comms established this and it is now shared by Knowledge and Artifacts. The
// reasoning, which is the part worth keeping:
//
// A view puts its selection in the URL, which is right — the view is
// copy-linkable and back/forward walks your reading order. What the URL cannot
// do is survive LEAVING. The nav rail sends you to `/knowledge`, which carries
// no selection, so the view's default-selection effect does exactly what it is
// written to do and lands you on the first space. Coming back from a board
// therefore lost the document you were reading.
//
// WHY THIS IS NOT "REMEMBER THE LAST HREF PER NAV SECTION". That was the first
// attempt and it cannot be made reliable: the href has to be written from the
// router's location, and sv-router updates `route.pathname` and `searchParams`
// at different points inside one navigation, so the recorder can see a real
// pathname beside the next page's empty search and store the bare path over the
// selection. Rewriting nav hrefs as you move also makes the rail itself feel
// unpredictable, which it did.
//
// A selection is not a URL and does not need the router at all. Each view
// derives its selection from its own location; this records THAT VALUE and
// hands it back when the view arrives with nothing selected. There is no moment
// in a navigation at which the two can disagree, because there is only one of
// them.
//
// SESSION-SCOPED, PER TAB: "where I was" is true of a sitting, not of a browser
// profile, and two tabs open on two documents should not fight over one key.
//
// TWO RULES FOR CALLERS, both learned the hard way:
//
//   1. WRITE ONLY WHILE YOU ARE STILL ON THE VIEW. A leaving view runs its
//      effects at least once against the next view's URL (see `isUnder` in
//      route-tabs.ts). Writing there records an empty selection over a good
//      memory.
//   2. RESTORE ONCE, ON ARRIVAL — not whenever the URL happens to look bare.
//      "Bare" is reachable from inside a view too: Artifacts' own "My files"
//      navigates to `/artifacts` with nothing selected, and a restore there
//      would drag the user back into the folder they just left. Arrival is a
//      MOUNT, so a latch on the effect is the honest test; the URL is not.

export interface ViewMemory<T> {
  /** The remembered value, or null if there is none. Reads from sessionStorage
   *  once per tab and mirrors it in memory afterwards. */
  read(): T | null
  write(value: T): void
  /** Tests only: drop the memory and the load latch. */
  reset(): void
}

/**
 * A session-scoped memory for one view's selection.
 *
 * `parse` is required rather than optional: what comes back out of storage is
 * whatever was in there, including a shape written by an older release, and a
 * selection that does not typecheck must degrade to "no memory" rather than
 * reach a view as a malformed object. `serialize` exists for the cases where
 * the stored shape is deliberately not the in-memory one.
 */
export function viewMemory<T>(
  key: string,
  parse: (raw: unknown) => T | null,
  serialize: (value: T) => unknown = (v) => v,
): ViewMemory<T> {
  // The in-memory mirror is not a cache for speed; it is what makes this work
  // in private mode, where sessionStorage throws on write. The memory still
  // serves the tab it was made in.
  let current: T | null = null
  let loaded = false

  return {
    read() {
      if (loaded) return current
      loaded = true
      try {
        const raw = window.sessionStorage.getItem(key)
        current = raw ? parse(JSON.parse(raw)) : null
      } catch {
        /* unreadable or malformed: no memory is a correct memory */
      }
      return current
    },
    write(value: T) {
      current = value
      loaded = true
      try {
        window.sessionStorage.setItem(key, JSON.stringify(serialize(value)))
      } catch {
        /* private mode: the in-memory mirror still serves this tab */
      }
    },
    reset() {
      current = null
      loaded = false
    },
  }
}
