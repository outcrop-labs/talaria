<script lang="ts">
  import type { NavItem } from '@/lib/nav'

  // Core items carry lucide components; enabled apps inject string glyphs from
  // their manifest — both render at the 16px icon slot.
  let { icon }: { icon: NavItem['icon'] } = $props()
</script>

{#if typeof icon === 'string'}
  <span class="grid h-4 w-4 place-items-center text-[13px] leading-none">{icon}</span>
{:else}
  {@const Icon = icon}
  <Icon size={16} strokeWidth={1.5} />
{/if}
