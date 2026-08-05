<script lang="ts">
  import { Smile } from '@lucide/svelte'
  import { cn } from '@/lib/cn'
  import { fade, listStagger, pop, POPOVER, QUICK } from '@/lib/motion'
  import Input from '@/components/ui/Input.svelte'
  import { popPanel, tileBase } from '@/components/chat/chat-chrome'
  import { EMOJI, searchEmoji } from '@/lib/emoji'

  // The composer's smiley button: a searchable grid, click to insert.
  let { onPick, disabled }: { onPick: (ch: string) => void; disabled?: boolean } = $props()

  let open = $state(false)
  let q = $state('')
  let wrapRef = $state<HTMLDivElement | null>(null)

  const results = $derived(q.trim() ? searchEmoji(q, 40) : EMOJI.slice(0, 40))
</script>

<svelte:document
  onmousedown={(e) => {
    if (open && !wrapRef?.contains(e.target as Node)) open = false
  }}
  onkeydown={(e) => {
    if (open && e.key === 'Escape') open = false
  }}
/>

<div bind:this={wrapRef} class="relative">
  <button
    type="button"
    title="Add emoji (or type :shortcode:)"
    {disabled}
    onclick={() => {
      open = !open
      q = ''
    }}
    class={tileBase}
  >
    <Smile size={15} />
  </button>
  {#if open}
    <div in:pop={POPOVER} out:fade={QUICK} class={cn(popPanel, 'absolute bottom-full left-0 z-30 mb-1.5 w-72 p-2')}>
      <Input autofocus size="sm" bind:value={q} placeholder="Search emoji" class="mb-2" />
      <div class="grid max-h-48 grid-cols-8 gap-0.5 overflow-y-auto" use:listStagger>
        {#each results as e (e.ch)}
          <button
            type="button"
            title={`:${e.names[0]}:`}
            onclick={() => {
              onPick(e.ch)
              open = false
            }}
            class="grid h-8 w-8 place-items-center rounded-md text-lg transition-colors hover:bg-hover"
          >
            {e.ch}
          </button>
        {/each}
        {#if results.length === 0}<div class="col-span-8 px-1 py-2 text-xs text-muted">No matches</div>{/if}
      </div>
    </div>
  {/if}
</div>
