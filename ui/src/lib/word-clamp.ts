const TRAILING = /[\s.,;:!?·(\[“"'–—-]+$/

export function wordStops(text: string): string[] {
  const stops: string[] = []
  for (const gap of text.matchAll(/\s+/g)) {
    const head = text.slice(0, gap.index).replace(TRAILING, '')
    if (head && `${head}…` !== stops.at(-1)) stops.push(`${head}…`)
  }
  return stops
}

/** `text` whole when it fits, else its longest run of whole words that does. Text whose first
 *  word alone does not fit comes back whole: it has no word boundary to end on, so the box cuts it.
 *  `fits` must only ever turn false as its candidate grows. */
export function clampWords(text: string, fits: (candidate: string) => boolean): string {
  if (fits(text)) return text
  const stops = wordStops(text)
  let low = 0
  let high = stops.length
  while (low < high) {
    const mid = (low + high) >> 1
    if (fits(stops[mid]!)) low = mid + 1
    else high = mid
  }
  return stops[low - 1] ?? text
}

export function wordClamp(node: HTMLElement, text: string): { update: (next: string) => void; destroy: () => void } {
  let full = text
  let width = -1
  const style = getComputedStyle(node)
  const edges = (): number => parseFloat(style.paddingLeft) + parseFloat(style.paddingRight) + parseFloat(style.borderLeftWidth) + parseFloat(style.borderRightWidth)
  const range = document.createRange()
  // The line is measured to the fraction of a pixel against the content box: CSS cuts a line
  // that is over by any amount, and `scrollWidth` rounds that amount away.
  const fits = (candidate: string): boolean => {
    node.textContent = candidate
    range.selectNodeContents(node)
    return range.getBoundingClientRect().width <= node.getBoundingClientRect().width - edges() + 0.01 && node.scrollHeight <= node.clientHeight + 1
  }
  const draw = (): void => {
    const shown = clampWords(full, fits)
    node.textContent = shown
    if (shown === full) node.removeAttribute('title')
    else node.title = full
    width = node.clientWidth
  }
  const observer = new ResizeObserver(() => {
    if (node.clientWidth !== width) draw()
  })
  node.textContent = full
  observer.observe(node)
  void document.fonts.ready.then(() => {
    if (node.isConnected) draw()
  })
  return {
    update(next) {
      full = next
      draw()
    },
    destroy: () => observer.disconnect(),
  }
}
