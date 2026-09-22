// Where a caret-anchored suggestion menu goes.
//
// mention-suggest.ts and slash-commands.ts each carried this verbatim: place the
// popup at the caret's rect, and flip it above the caret when it would overflow
// the viewport bottom (and would still fit above). The two menus are for two
// chars of the same editor, so the placement rule is one rule.

/** Position a `position: fixed` popup at `rect`, flipping above when the space
 *  below is not enough and the space above is. */
export function placeMenu(el: HTMLElement, rect: DOMRect): void {
  const margin = 6
  el.style.left = `${rect.left}px`
  const below = rect.bottom + margin
  const wouldOverflow = below + el.offsetHeight > window.innerHeight
  if (wouldOverflow && rect.top - margin - el.offsetHeight > 0) {
    el.style.top = `${rect.top - margin - el.offsetHeight}px`
  } else {
    el.style.top = `${below}px`
  }
}
