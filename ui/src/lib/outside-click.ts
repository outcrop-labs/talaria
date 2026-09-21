// The outside-pointer test both sanctioned shells make.
//
// Popover.svelte and DropdownMenu.svelte each wrote the same four lines: a
// document `mousedown` closes the panel unless the event landed inside the
// trigger (`ref`) or inside the panel. It is the one part of an outside-close
// engine that must not differ between them — the other three decisions (Escape,
// scroll, stacking) are what the shells exist to answer differently.

/** True when a document-level mousedown landed outside both the trigger and
 *  the panel — i.e. "this click closes us". A missing element is not a match:
 *  a panel that has not mounted yet must not swallow the click that opens it. */
export function outsidePointer(
  e: MouseEvent,
  ref: HTMLElement | undefined | null,
  panel: HTMLElement | undefined | null,
): boolean {
  const t = e.target as Node
  return !ref?.contains(t) && !panel?.contains(t)
}
