<script lang="ts">
  import { Link2, Copy, Check } from '@lucide/svelte'
  import { cn } from '@/lib/cn'
  import { fade } from '@/lib/motion'
  import { focusRing } from './control'

  // The one copy-to-clipboard affordance: icon (or icon+label) that flashes a
  // check. Pass `value` for literal text, or `path` for an app link (origin
  // resolved at click time, SSR-safe). Stops propagation so it never triggers
  // the surrounding card/row click.
  let {
    value,
    path,
    label,
    class: className,
    title = 'Copy',
  }: {
    /** Literal text to copy. */
    value?: string
    /** App path — copied as a full URL. */
    path?: string
    label?: string
    class?: string
    title?: string
  } = $props()

  let copied = $state(false)
</script>

<button data-dither-fill
  type="button"
  {title}
  aria-label={title}
  onclick={(e) => {
    e.preventDefault()
    e.stopPropagation()
    const text = value ?? (path ? window.location.origin + path : '')
    if (!text) return
    void navigator.clipboard?.writeText(text)
    copied = true
    setTimeout(() => (copied = false), 1200)
  }}
  class={cn(
    // Ghost icon affordance (spec §8): muted → readout, hover-token fill.
    'flex items-center gap-1 rounded-md p-1 text-muted transition-colors hover:text-fg',
    focusRing,
    className,
  )}
>
  {#if copied}
    <!-- The check fades in (the confirm moment); the revert swap stays instant. -->
    <span in:fade={{ duration: 150 }} class="flex items-center"><Check size={13} /></span>
  {:else if path}
    <Link2 size={13} />
  {:else}
    <Copy size={13} />
  {/if}
  {#if label}<span>{copied ? 'Copied' : label}</span>{/if}
</button>
