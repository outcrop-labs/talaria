<script lang="ts">
  import { cn } from '@/lib/cn'

  // Small square avatar for flattened message rows (spec §10 / §8): raised
  // tile, strong ring, mono initials.
  let { name, class: className }: { name?: string | null; class?: string } = $props()

  const initials = $derived.by(() => {
    const words = (name ?? '').trim().split(/\s+/).filter(Boolean)
    return words.length >= 2 ? `${words[0]![0]}${words[1]![0]}` : (words[0]?.slice(0, 2) ?? '?')
  })
</script>

<span
  class={cn(
    'grid h-6 w-6 shrink-0 place-items-center rounded border border-line-strong bg-raised font-mono text-[10px] font-medium uppercase tracking-[0.05em] text-muted',
    className,
  )}
>
  {initials}
</span>
