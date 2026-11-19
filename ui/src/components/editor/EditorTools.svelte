<script lang="ts">
  import { History, Sparkles } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import { cn } from '@/lib/cn'

  // The tools of a document being edited in the settings area, at the right
  // of its title line: Draft with Muse, then History. Every editor puts them
  // here, so the foot of each holds only its Delete and its Save.
  let {
    muse,
    history,
    class: className,
  }: {
    /** `title` says what the draft covers (the text, or every field). */
    muse?: { open: boolean; onToggle: () => void; title: string }
    history?: { open: boolean; onToggle: () => void }
    class?: string
  } = $props()
</script>

{#if muse || history}
  <div class={cn('ml-auto flex shrink-0 items-center', className)}>
    {#if muse}
      <Button
        variant="ghost"
        size="sm"
        class={cn('gap-1.5', !history && '-mr-3', muse.open && 'text-fg')}
        aria-expanded={muse.open}
        onclick={muse.onToggle}
        title={muse.title}
      >
        <Sparkles size={14} />Draft with Muse
      </Button>
    {/if}
    {#if history}
      <Button variant="ghost" size="sm" class={cn('-mr-3 gap-1.5', history.open && 'text-fg')} aria-pressed={history.open} onclick={history.onToggle}>
        <History size={14} />History
      </Button>
    {/if}
  </div>
{/if}
