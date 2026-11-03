<script lang="ts">
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import type { NavItem } from '@/lib/nav'
  import { swap } from '@/lib/motion'
  import { GRID_SCALE } from '@/lib/waiting/scale'
  import { waitingFor } from '@/lib/waiting/session.svelte'
  import type { WaitingSiteKey } from '@/lib/waiting/sites'
  import NavIcon from './NavIcon.svelte'

  /**
   * A view's face in the bar: its icon, or, while an agent works there, its
   * waiting mark in the same square. Both sit in one grid cell, so the change
   * reads as one object turning over.
   *
   * The mark fills `size + 2` square. WaitingMark scales a grid up by
   * GRID_SCALE to match text weight, which would push it past the icon's
   * room, so a grid is asked for that much less.
   */
  let { icon, size, site }: { icon: NavItem['icon']; size: number; site: WaitingSiteKey | null } = $props()

  const box = $derived(size + 2)
  const markSize = $derived(site && waitingFor(site).state.family === 'grid' ? box / GRID_SCALE : box)
</script>

<span class="grid shrink-0 place-items-center" style:width="{box}px" style:height="{box}px">
  {#if site}
    <span class="grid place-items-center [grid-area:1/1]" in:swap out:swap={{ duration: 120 }}>
      <WaitingMark {site} size={markSize} class="text-accent" />
    </span>
  {:else}
    <span class="grid place-items-center [grid-area:1/1]" in:swap out:swap={{ duration: 120 }}>
      <NavIcon {icon} {size} />
    </span>
  {/if}
</span>
