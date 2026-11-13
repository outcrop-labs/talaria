<script lang="ts">
  import { cn } from '@/lib/cn'
  import { initials } from '@/lib/initials'
  import { useUiVoice } from '@/lib/ui-voice'

  let {
    src,
    name,
    class: className,
  }: {
    src?: string | null
    name?: string | null
    class?: string
  } = $props()

  const sans = useUiVoice() === 'sans'

  const cls = $derived(cn('h-7 w-7 shrink-0 rounded-full border border-line-strong', className))
</script>

<!-- Avatar per spec §8: round raised tile, strong hairline ring, mono
     initials. Reuse for user/agent avatars. -->
{#if src}
  <img {src} alt="" class={cn(cls, 'object-cover')} />
{:else}
  <span
    class={cn(
      cls,
      sans
        ? 'grid place-items-center bg-raised font-sans text-xs font-medium text-muted'
        : 'grid place-items-center bg-raised font-mono text-[10px] font-medium uppercase tracking-[0.05em] text-muted',
    )}
  >
    {sans ? initials(name).toUpperCase() : initials(name)}
  </span>
{/if}
