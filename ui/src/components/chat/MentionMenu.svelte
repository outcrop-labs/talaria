<script lang="ts">
  import { cn } from '@/lib/cn'
  import { fade, QUICK } from '@/lib/motion'
  import Avatar from '@/components/ui/Avatar.svelte'
  import { popPanel, popRow } from '@/components/chat/chat-chrome'
  import type { MentionState } from './mentions.svelte'

  // The mention dropdown panel. Position it with `class` (host-specific).
  let {
    mention,
    picked,
    onPick,
    class: className,
  }: {
    mention: MentionState
    picked: number
    onPick: (insert: string) => void
    class?: string
  } = $props()
</script>

<div out:fade={QUICK} class={cn(popPanel, 'z-10 w-64 overflow-hidden', className)}>
  {#each mention.options as a, i (`${a.insert}-${a.sub ?? ''}`)}
    <button
      type="button"
      onmousedown={(e) => {
        e.preventDefault()
        onPick(a.insert)
      }}
      class={cn(popRow, i === picked ? 'bg-hover text-fg' : 'text-muted')}
    >
      <Avatar name={a.label} class="h-5 w-5 text-xs" />
      <span class="truncate">{a.label}</span>
      {#if a.sub}
        <span class="ml-auto max-w-28 truncate font-mono text-[10px] tracking-[0.05em] text-ink-dim">{a.sub}</span>
      {/if}
    </button>
  {/each}
</div>
