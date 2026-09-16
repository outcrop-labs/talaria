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
    <Sparkles size={14} class={cn('mb-3 shrink-0 text-accent', busy && 'gd-pulse')} />
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
  {#if busy}
    <!-- THE IN-PROGRESS STATE: a whole-agent refine is a JSON contract, so
         there is no token stream to show (the server validates and answers with
         the finished draft). What is left to say is that it IS running and for
         how long — the same honest "working, slowly" signal the describe step
         shows. The disabled button alone was the silence Jon hit. -->
    <div in:slide={{ duration: 150 }} class="rounded-md border border-line bg-surface px-2.5 py-2" data-refine-inflight="visible">
      <div class="flex items-center gap-2 text-xs text-muted">
        <span class="gd-pulse text-accent">▍</span>
        <span>Refining the design: identity, soul, and starter skills{preview ? ` — ${preview}` : ''}</span>
      </div>
    </div>
  {/if}
  {#if error}<p transition:slide={{ duration: 150 }} class="text-xs text-danger">{error}</p>{/if}
</div>
