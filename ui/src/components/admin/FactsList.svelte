<script lang="ts">
  import { cn } from '@/lib/cn'
  import type { Fact } from './facts'

  // Read-only facts as a row-shaped block of a `SettingsGroup`. A dialog
  // passes `class="p-0"` to sit them in its own padding.
  let { facts, class: className }: { facts: Fact[]; class?: string } = $props()
</script>

<dl class={cn('grid grid-cols-[10rem_minmax(0,1fr)] gap-x-4 gap-y-2 px-5 py-4', className)}>
  {#each facts as f (f.label)}
    <dt class="font-sans text-[13px] leading-5 text-muted">{f.label}</dt>
    <dd class={f.mono ? 'min-w-0 break-all font-mono text-xs leading-5 text-fg' : 'min-w-0 font-sans text-[13px] leading-5 text-fg'}>{f.value}</dd>
  {/each}
</dl>
