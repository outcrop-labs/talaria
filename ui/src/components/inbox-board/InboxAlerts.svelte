<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import { cn } from '@/lib/cn'
  import { needsBreakdown, type InboxCard, type InboxGlyph } from '@/lib/inbox-board'
  import StateGlyph from './StateGlyph.svelte'
  import { ACTION } from './inbox-board.svelte'

  let {
    needs,
    tally,
    calm,
    onRing,
  }: {
    needs: readonly InboxCard[]
    tally: string | null
    /** Nothing in Needs you and nobody waiting on a reply. */
    calm: boolean
    onRing: (glyph: InboxGlyph) => void
  } = $props()

  const kinds = $derived(needsBreakdown(needs))

  const DANGER = 'border-danger/35 bg-[var(--inbox-chip-danger)] hover:border-danger/70 hover:bg-[var(--inbox-chip-danger)]'
  const AMBER = 'border-warning/40 bg-[var(--inbox-chip-warning)] hover:border-warning/80 hover:bg-[var(--inbox-chip-warning)]'
  const WASH: Partial<Record<InboxGlyph, string>> = { failed: DANGER, blocked: DANGER, signoff: AMBER, waiting: AMBER }
</script>

{#if calm}
  <p class="border-t border-line px-5 py-2.5 font-sans text-[13px] leading-6 tabular-nums text-muted">
    <span class="font-medium text-fg">Nothing needs you</span>{#if tally}{' · '}{tally}{/if}
  </p>
{:else if kinds.length > 0 || tally}
  <div class="flex flex-wrap items-center gap-2 border-t border-line px-5 py-2.5">
    {#each kinds as kind (kind.glyph)}
      <Button
        {...ACTION}
        variant="outline"
        title="Show these cards"
        class={cn('h-6 gap-1.5 px-2.5 text-fg', WASH[kind.glyph] ?? 'border-line bg-surface')}
        onclick={() => onRing(kind.glyph)}
      >
        <StateGlyph glyph={kind.glyph} />
        {kind.word}
        <span class="font-normal tabular-nums text-muted">{kind.count}</span>
      </Button>
    {/each}
    {#if tally}<p class={cn('font-sans text-[13px] leading-6 tabular-nums text-muted', kinds.length > 0 && 'ml-auto')}>{tally}</p>{/if}
  </div>
{/if}
