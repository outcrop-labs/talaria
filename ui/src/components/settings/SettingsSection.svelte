<script lang="ts">
  import type { Snippet } from 'svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import { cn } from '@/lib/cn'

  // A titled block of a settings page: an h2, an optional sentence, then the
  // section's card.
  let {
    title,
    description,
    info,
    action,
    class: className,
    children,
  }: {
    title: string
    /** One sentence under the title. */
    description?: string
    /** The longer explanation, behind an InfoTip beside the title. */
    info?: string
    /** Right aligned on the title row. */
    action?: Snippet
    class?: string
    children: Snippet
  } = $props()
</script>

<section class={className}>
  <div class={cn('mb-3 flex gap-4', description ? 'items-end' : 'items-center')}>
    <div class="min-w-0 flex-1">
      <div class="flex items-center gap-2">
        <h2 class="font-sans text-[15px] font-semibold leading-5 text-fg">{title}</h2>
        {#if info}<InfoTip text={info} />{/if}
      </div>
      {#if description}<p class="mt-1 font-sans text-[13px] leading-5 text-muted">{description}</p>{/if}
    </div>
    {#if action}<div class="flex shrink-0 items-center gap-2">{@render action()}</div>{/if}
  </div>
  {@render children()}
</section>
