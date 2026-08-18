<script lang="ts">
  import type { Snippet } from 'svelte'
  import { Plus } from '@lucide/svelte'
  import { cn } from '@/lib/cn'
  import { focusGold } from '@/components/chat/chat-chrome'
  import { listStagger, slide } from '@/lib/motion'
  import Input from '@/components/ui/Input.svelte'
  import Materialize from '@/components/ui/Materialize.svelte'

  // Sidebar section: the create affordance is a small "+" IN the heading (Slack-
  // style) that expands to an inline name input — no chunky buttons under lists.
  // Header = the §8 canonical: 10px mono uppercase 0.08em ink-dim, right-aligned
  // mono meta (live row count, `CHANNELS … 08`).
  let {
    label,
    meta,
    createPlaceholder,
    onCreate,
    loading = false,
    count = 3,
    rowSkeleton,
    children,
  }: {
    label: string
    /** Right-aligned mono meta — real counts only, no fabricated data. */
    meta?: string
    createPlaceholder?: string
    onCreate?: (name: string) => void
    /** First load of the query behind this section — row-shaped skeletons
     *  materialize into the rows (Materialize) instead of a branch swap. */
    loading?: boolean
    count?: number
    /** One row's silhouette, mirroring this section's RailRow anatomy. */
    rowSkeleton?: Snippet<[number]>
    children: Snippet
  } = $props()

  let creating = $state(false)
  let name = $state('')
  const submit = (cancelled: boolean) => {
    const v = name.trim()
    creating = false
    name = ''
    if (v && !cancelled) onCreate?.(v)
  }
</script>

<div class="mb-4">
  <div class="mb-1 flex h-6 items-center gap-1.5 px-2">
    <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">{label}</span>
    <span class="ml-auto"></span>
    {#if meta}<span class="font-mono text-[10px] tracking-[0.05em] text-muted">{meta}</span>{/if}
    {#if onCreate}
      <button
        type="button"
        title={`New ${label.toLowerCase().replace(/s$/, '')}`}
        onclick={() => (creating = true)}
        class={cn(
          'grid h-5 w-5 place-items-center rounded-md text-muted transition-colors hover:dither-fill hover:text-fg',
          focusGold,
        )}
      >
        <Plus size={13} />
      </button>
    {/if}
  </div>
  {#if creating}
    <div transition:slide={{ duration: 150 }} class="mb-1 px-1">
      <!-- svelte-ignore a11y_autofocus -->
      <Input
        autofocus
        size="sm"
        bind:value={name}
        placeholder={createPlaceholder}
        onkeydown={(e) => {
          if (e.key === 'Enter') submit(false)
          else if (e.key === 'Escape') submit(true)
        }}
        onblur={() => submit(false)}
      />
    </div>
  {/if}
  <!-- Any grid or list staggers its items on mount (ANIMATIONS.md). With a
       rowSkeleton the section renders through Materialize: row-shaped
       skeletons occupy the same stack and the real rows stagger in over them
       (its content branch owns the cascade). Rows are divs (see RailRow). -->
  {#if rowSkeleton}
    <Materialize {loading} {count} class="space-y-0.5">
      {#snippet skeleton(i)}{@render rowSkeleton(i)}{/snippet}
      {@render children()}
    </Materialize>
  {:else}
    <div use:listStagger class="space-y-0.5">{@render children()}</div>
  {/if}
</div>
