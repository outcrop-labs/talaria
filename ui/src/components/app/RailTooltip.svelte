<script lang="ts">
  import type { Snippet } from 'svelte'
  import { portal } from '@/lib/portal'

  // Rail tooltips render into a portal at a fixed position so the rail's
  // scroll container can't clip them.
  let { label, children }: { label: string; children: Snippet } = $props()

  let pos = $state<{ x: number; y: number } | null>(null)
</script>

<!-- svelte-ignore a11y_no_static_element_interactions, a11y_click_events_have_key_events, a11y_mouse_events_have_key_events -->
<div
  onmouseenter={(e) => {
    const r = e.currentTarget.getBoundingClientRect()
    pos = { x: r.right + 8, y: r.top + r.height / 2 }
  }}
  onmouseleave={() => (pos = null)}
  onclick={() => (pos = null)}
>
  {@render children()}
  {#if pos}
    <div
      use:portal
      style:left="{pos.x}px"
      style:top="{pos.y}px"
      class="pointer-events-none fixed z-50 -translate-y-1/2 whitespace-nowrap rounded-md border border-line bg-raised px-2 py-1 font-mono text-[10px] uppercase tracking-[0.05em] text-fg"
    >
      {label}
    </div>
  {/if}
</div>
