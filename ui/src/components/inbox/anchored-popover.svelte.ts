// Shared plumbing for the composer-rail popovers (port of the React
// useAnchoredPopover hook — runes, so this is a .svelte.ts module): anchors
// the portaled panel to the trigger button's bottom-left corner (6px gap),
// re-places it on resize/scroll, and dismisses on outside mousedown. Call it
// during component init; bind the trigger/panel elements to `.button`/`.panel`
// and read `.position` (null until placed).
export interface AnchoredPopoverPosition {
  left: number
  bottom: number
}

export function createAnchoredPopover(open: () => boolean, setOpen: (open: boolean) => void) {
  let button = $state<HTMLButtonElement | null>(null)
  let panel = $state<HTMLDivElement | null>(null)
  let position = $state<AnchoredPopoverPosition | null>(null)

  $effect(() => {
    if (!open()) return
    const place = () => {
      const rect = button?.getBoundingClientRect()
      if (rect) position = { left: rect.left, bottom: window.innerHeight - rect.top + 6 }
    }
    const dismiss = (event: MouseEvent) => {
      const target = event.target as Node
      if (!button?.contains(target) && !panel?.contains(target)) {
        setOpen(false)
      }
    }
    place()
    window.addEventListener('resize', place)
    window.addEventListener('scroll', place, true)
    document.addEventListener('mousedown', dismiss)
    return () => {
      window.removeEventListener('resize', place)
      window.removeEventListener('scroll', place, true)
      document.removeEventListener('mousedown', dismiss)
    }
  })

  return {
    get button() {
      return button
    },
    set button(node) {
      button = node
    },
    get panel() {
      return panel
    },
    set panel(node) {
      panel = node
    },
    get position() {
      return position
    },
  }
}
