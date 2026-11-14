<script lang="ts">
  import { fact, factLink } from '@/components/observability/grammar'
  import { cn } from '@/lib/cn'
  import Skeleton from '@/components/ui/Skeleton.svelte'

  // One count of the overview's inventory line: "6 apps", a link to the page
  // that owns them, and after it the one thing wrong with them today, if any,
  // as the next fact of the line so one separator runs through it.
  let {
    href,
    count,
    noun,
    plural = `${noun}s`,
    note,
    failed = false,
  }: {
    href: string
    /** Undefined until the read lands. */
    count: number | undefined
    noun: string
    plural?: string
    /** An exception, in words: "1 build failed". */
    note?: string
    /** The read failed, so there is no count to show. */
    failed?: boolean
  } = $props()
</script>

<span class={fact}>
  {#if count !== undefined}
    <a {href} class={factLink}><span class="tabular-nums text-fg">{count.toLocaleString('en-US')}</span> {count === 1 ? noun : plural}</a>
  {:else if failed}
    <a {href} class={factLink}>{plural.charAt(0).toUpperCase() + plural.slice(1)} unavailable</a>
  {:else}
    <Skeleton class="inline-block h-3 w-14 rounded-full align-middle" />
  {/if}
</span>
{#if count !== undefined && note}<span class={cn(fact, 'text-fg')}>{note}</span>{/if}
