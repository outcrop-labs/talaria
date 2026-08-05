/** Svelte action: move the node to <body> (or a given target). `position:
 *  fixed` is relative to the nearest transformed/filtered ancestor (some
 *  surfaces carry backdrop-filter), which would otherwise center a modal
 *  inside a card rather than the viewport — the portal escapes any such
 *  containing block. */
export function portal(node: HTMLElement, target: HTMLElement | undefined = undefined) {
  ;(target ?? document.body).appendChild(node)
  return {
    destroy() {
      node.remove()
    },
  }
}
