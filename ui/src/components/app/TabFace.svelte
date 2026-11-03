<script lang="ts">
  import WaitingMark from '@/components/ui/WaitingMark.svelte'
  import type { NavItem } from '@/lib/nav'
  import { swap } from '@/lib/motion'
  import { watchTabMark } from '@/lib/tab-mark-on-screen'
  import { GRID_SCALE } from '@/lib/waiting/scale'
  import { waitingFor } from '@/lib/waiting/session.svelte'
  import type { WaitingSiteKey } from '@/lib/waiting/sites'
  import NavIcon from './NavIcon.svelte'

  let {
    icon,
    size,
    site,
    tab = false,
    still = false,
  }: {
    icon: NavItem['icon']
    size: number
    site: WaitingSiteKey | null
    /** This face is the tab's own, so whether its mark is on screen is reported. */
    tab?: boolean
    /** Hold the mark still: the same mark is moving elsewhere on screen. */
    still?: boolean
  } = $props()

  const box = $derived(size + 2)
  const sizeBeforeGridScale = $derived(site && waitingFor(site).state.family === 'grid' ? box / GRID_SCALE : box)
</script>

<span class="grid shrink-0 place-items-center" style:width="{box}px" style:height="{box}px">
  {#if site}
    <span class="grid place-items-center [grid-area:1/1]" in:swap out:swap={{ duration: 120 }} {@attach tab && watchTabMark(site)}>
      <WaitingMark {site} {still} size={sizeBeforeGridScale} class="text-accent" />
    </span>
  {:else}
    <span class="grid place-items-center [grid-area:1/1]" in:swap out:swap={{ duration: 120 }}>
      <NavIcon {icon} {size} />
    </span>
  {/if}
</span>
