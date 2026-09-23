<script lang="ts">
  import { BookOpen, FileText, Hash, Kanban, Map, Search, Ticket } from '@lucide/svelte'
  import { chipTitle, entityLabel, type ChipEntity } from '@/lib/chips'
  import { focusGold } from '@/components/chat/chat-chrome'
  import { cn } from '@/lib/cn'

  // A platform link as a compact chip: icon + title, never the raw URL.
  // In-app anchor (no target=_blank) so the SPA router keeps the click.
  let {
    href,
    entity,
    title,
  }: {
    href: string
    entity: ChipEntity
    title?: string
  } = $props()

  const label = $derived(chipTitle({ entity, title }))
  const kind = $derived(entityLabel(entity))
</script>

<a
  {href}
  class={cn(
    'mx-0.5 inline-flex max-w-full items-center gap-1.5 rounded-md border border-line bg-raised px-2 py-0.5 align-middle font-sans text-xs text-fg transition-colors hover:border-line-strong hover:bg-hover',
    focusGold,
  )}
  title={label === kind ? kind : `${kind}: ${label}`}
>
  <span class="shrink-0 text-muted" aria-hidden="true">
    {#if entity === 'board'}<Kanban size={13} />
    {:else if entity === 'ticket'}<Ticket size={13} />
    {:else if entity === 'kb'}<BookOpen size={13} />
    {:else if entity === 'document'}<FileText size={13} />
    {:else if entity === 'channel'}<Hash size={13} />
    {:else if entity === 'plan'}<Map size={13} />
    {:else}<Search size={13} />
    {/if}
  </span>
  <span class="max-w-52 truncate">{label}</span>
</a>
