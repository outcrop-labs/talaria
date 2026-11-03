<script lang="ts">
  import type { NavItem } from '@/lib/nav'

  // Core items carry lucide components; enabled apps inject string glyphs from
  // their manifest — both render at the same icon slot, 16px unless told.
  let { icon, size = 16 }: { icon: NavItem['icon']; size?: number } = $props()
</script>

{#if typeof icon === 'string'}
  <span
    class="grid place-items-center leading-none"
    style:width="{size}px"
    style:height="{size}px"
    style:font-size="{size - 3}px">{icon}</span
  >
{:else}
  {@const Icon = icon}
  <Icon {size} strokeWidth={1.5} />
{/if}
