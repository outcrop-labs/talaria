<script lang="ts">
  import { File, FileText, ListChecks, PanelsTopLeft, ScrollText, Table, Telescope } from '@lucide/svelte'
  import { cn } from '@/lib/cn'
  import type { TabGlyph } from '@/lib/workspace-tabs.svelte'

  // THE SMALL TYPE MARK a session file wears on its tab and in its header —
  // lucide, because it sits inside clickable chrome (UI-CONVENTIONS: the action
  // register). A Google file wears a mono "G" instead of a type icon: what
  // matters first about it is WHERE it lives, and the header's label says the
  // type in words.
  let { glyph, size = 12, class: className }: { glyph: TabGlyph; size?: number; class?: string } = $props()

  const ICONS = {
    doc: FileText,
    sheet: Table,
    page: PanelsTopLeft,
    file: File,
    plan: ScrollText,
    report: Telescope,
    drafts: ListChecks,
  } as const
  const google = $derived(glyph.startsWith('google-'))
  const Icon = $derived(google ? null : ICONS[glyph as keyof typeof ICONS])
</script>

{#if google}
  <span
    aria-hidden="true"
    class={cn('grid shrink-0 place-items-center rounded-[3px] border border-current font-mono font-semibold leading-none', className)}
    style:width={`${size}px`}
    style:height={`${size}px`}
    style:font-size={`${Math.max(7, size - 4)}px`}>G</span
  >
{:else if Icon}
  <Icon {size} aria-hidden="true" class={cn('shrink-0', className)} />
{/if}
