// Marks a scrolling tab strip with `data-more` while tabs remain off to the
// right, so the trailing fade (styles.css `.tab-strip[data-more]`) shows only
// then: not on a strip that fits, and not at the end of the scroll, where a
// fade would dim the tab you scrolled to.
export function edgeFade(node: HTMLElement) {
  const update = () => {
    const more = node.scrollWidth - node.clientWidth - node.scrollLeft > 1
    if (more) node.dataset.more = ''
    else delete node.dataset.more
  }
  // Opening a tab swaps a tile for a wider element, so the strip's own size
  // can hold still while its content grows. Watch every child, and re-observe
  // whenever the set of children changes.
  const ro = new ResizeObserver(update)
  const observeAll = () => {
    ro.disconnect()
    ro.observe(node)
    for (const child of node.children) ro.observe(child)
    update()
  }
  const mo = new MutationObserver(observeAll)
  mo.observe(node, { childList: true })
  node.addEventListener('scroll', update, { passive: true })
  observeAll()
  return {
    destroy() {
      ro.disconnect()
      mo.disconnect()
      node.removeEventListener('scroll', update)
    },
  }
}
