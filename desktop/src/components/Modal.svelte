<script lang="ts" module>
  // The element to give focus back to on close. Recorded on every focusin,
  // because by the time a modal mounts the background is already `inert`,
  // and making an element inert blurs it: document.activeElement is BODY.
  let lastFocus: HTMLElement | null = null
  if (typeof document !== 'undefined') {
    document.addEventListener('focusin', (e) => {
      const el = e.target
      if (el instanceof HTMLElement && !el.closest('[role=dialog]')) lastFocus = el
    })
  }
</script>

<script lang="ts">
  // The launcher's modal frame: focus moves in (to `[data-autofocus]`, else
  // the first control), Tab and Shift+Tab stay inside, Escape calls
  // `onescape`, and focus goes back where it was on close. App.svelte makes
  // everything behind it `inert`, so the instance rows and titlebar buttons
  // are out of reach while it is up.
  import type { Snippet } from 'svelte'

  let {
    label,
    onescape,
    children,
    class: className = '',
  }: { label: string; onescape: () => void; children: Snippet; class?: string } = $props()

  let box = $state<HTMLElement | null>(null)

  const FOCUSABLE =
    'button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [href], [tabindex]:not([tabindex="-1"])'

  const focusables = () =>
    box ? [...box.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((el) => el.offsetParent !== null) : []

  $effect(() => {
    if (!box) return
    const previous = lastFocus
    const first = box.querySelector<HTMLElement>('[data-autofocus]') ?? focusables()[0]
    first?.focus()
    return () => {
      // After the flush that lifts `inert`: an inert element cannot take focus.
      setTimeout(() => {
        if (previous?.isConnected && !previous.closest('[inert]')) previous.focus()
      }, 0)
    }
  })

  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault()
      e.stopPropagation()
      onescape()
      return
    }
    if (e.key !== 'Tab' || !box) return
    const items = focusables()
    const first = items[0]
    const last = items[items.length - 1]
    if (!first || !last) {
      e.preventDefault()
      return
    }
    const at = document.activeElement
    const inside = at instanceof Node && box.contains(at)
    if (e.shiftKey && (at === first || !inside)) {
      e.preventDefault()
      last.focus()
    } else if (!e.shiftKey && (at === last || !inside)) {
      e.preventDefault()
      first.focus()
    }
  }
</script>

<svelte:window {onkeydown} />

<div
  class="fixed inset-0 z-50 grid place-items-center bg-black/60 p-4"
  role="dialog"
  aria-modal="true"
  aria-label={label}
>
  <div bind:this={box} class={className}>
    {@render children()}
  </div>
</div>
