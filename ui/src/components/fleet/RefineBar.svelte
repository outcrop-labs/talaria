<script lang="ts">
  import { Sparkles } from '@lucide/svelte'
  import Button from '@/components/ui/Button.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import { slide } from '@/lib/motion'
  import { cn } from '@/lib/cn'

  let {
    busy,
    preview,
    error,
    onRefine,
  }: {
    busy: boolean
    preview: string | null
    error: string | null
    onRefine: (text: string) => void
  } = $props()

  let text = $state('')
</script>

<div class={cn('space-y-2 rounded-lg border border-line p-3.5', busy && 'opacity-90')}>
  <div class="flex items-end gap-2.5">
    <Sparkles size={14} class="mb-3 shrink-0 text-accent" />
    <Textarea
      autoGrow
      rows={1}
      bind:value={text}
      onkeydown={(e) => {
        if (e.key === 'Enter' && !e.shiftKey) {
          e.preventDefault()
          if (!busy && text.trim()) {
            onRefine(text)
            text = ''
          }
        }
      }}
      placeholder="Refine the design, e.g. “more formal, and add a weekly retro skill”"
      class="max-h-32 text-sm"
    />
    <Button
      variant="outline"
      class="shrink-0"
      disabled={busy || !text.trim()}
      onclick={() => {
        onRefine(text)
        text = ''
      }}
    >
      {busy ? 'Refining' : 'Refine'}
    </Button>
  </div>
  {#if busy && preview !== null}
    <pre class="max-h-32 overflow-y-auto whitespace-pre-wrap rounded-md border border-line bg-surface p-2.5 font-mono text-[11px] leading-4 text-muted">{preview || 'Designing'}<span class="gd-pulse text-accent">▍</span></pre>
  {/if}
  {#if error}<p transition:slide={{ duration: 150 }} class="text-xs text-danger">{error}</p>{/if}
</div>
