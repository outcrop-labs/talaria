// Marks a scrolling tab strip with `data-more` while tabs remain off to the
// right, so the trailing fade (styles.css `.tab-strip[data-more]`) shows only
// then.
export function edgeFade(node: HTMLElement) {
  const update = () => {
    const more = node.scrollWidth - node.clientWidth - node.scrollLeft > 1
    if (more) node.dataset.more = ''
    else delete node.dataset.more
  }
  // ResizeObserver reports border-box changes only, so a child growing inside
  // a strip of fixed size is seen only by observing each child.
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

// Room kept beside the open tab when it is scrolled into view: its feet on
// the left, its feet plus the trailing fade on the right.
const LEAD = 12
const TRAIL = 32

// Scrolls a narrow strip so the open tab (`aria-current="page"`) is in view on
// mount and whenever the open tab changes, which swaps the strip's children.
export function revealCurrent(node: HTMLElement) {
  const reveal = () => {
    const tab = node.querySelector<HTMLElement>('[aria-current="page"]')
    if (!tab) return
    const strip = node.getBoundingClientRect()
    const box = tab.getBoundingClientRect()
    if (box.left - LEAD < strip.left) node.scrollLeft -= strip.left - (box.left - LEAD)
    else if (box.right + TRAIL > strip.right) node.scrollLeft += box.right + TRAIL - strip.right
  }
  const mo = new MutationObserver(reveal)
  mo.observe(node, { childList: true })
  reveal()
  return {
    destroy() {
      mo.disconnect()
    },
  }
}
