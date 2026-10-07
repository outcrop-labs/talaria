<script lang="ts">
  import { cn } from '@/lib/cn'

  // Small square avatar for flattened message rows (spec §10 / §8): raised
  // tile, strong ring, mono initials — or the author's photo when one exists
  // (R3). A photo that fails to load falls back to the initials rather than a
  // broken-image glyph.
  let {
    name,
    src,
    class: className,
  }: { name?: string | null; src?: string | null; class?: string } = $props()

  const initials = $derived.by(() => {
    const words = (name ?? '').trim().split(/\s+/).filter(Boolean)
    return words.length >= 2 ? `${words[0]![0]}${words[1]![0]}` : (words[0]?.slice(0, 2) ?? '?')
  })

  // Keyed to the src it failed for, so a new photo URL gets a fresh try.
  let failedSrc = $state<string | null>(null)
  const showImg = $derived(!!src && failedSrc !== src)
  const base = 'h-6 w-6 shrink-0 rounded border border-line-strong bg-raised'
</script>

{#if showImg}
  <img
    {src}
    alt=""
    referrerpolicy="no-referrer"
    class={cn(base, 'object-cover', className)}
    onerror={() => (failedSrc = src ?? null)}
  />
{:else}
  <span
    class={cn(
      base,
      'grid place-items-center font-mono text-[10px] font-medium uppercase tracking-[0.05em] text-muted',
      className,
    )}
  >
    {initials}
  </span>
{/if}
