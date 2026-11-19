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

<div class={cn('space-y-2', busy && 'opacity-90')}>
  <div class="flex items-start gap-2">
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
      aria-label="Refine the design"
      placeholder="Refine the design, e.g. “more formal, and add a weekly retro skill”"
      class="max-h-32 py-[7px]"
    />
    <Button
      variant="outline"
      size="md"
      class="shrink-0"
      disabled={busy || !text.trim()}
      onclick={() => {
        onRefine(text)
        text = ''
      }}
    >
      <Sparkles size={14} class={cn(busy && 'gd-pulse')} />
      {busy ? 'Refining' : 'Refine'}
    </Button>
  </div>
  {#if busy}
    <!-- THE IN-PROGRESS STATE: a whole-agent refine is a JSON contract, so
         there is no token stream to show (the server validates and answers with
         the finished draft). What is left to say is that it IS running. The
         disabled button alone was the silence Jon hit. -->
    <p in:slide={{ duration: 150 }} class="flex items-center gap-2 font-sans text-[13px] leading-5 text-muted" data-refine-inflight="visible">
      <span class="gd-pulse text-accent">▍</span>
      Refining the design{preview ? `: ${preview}` : ''}
    </p>
  {/if}
  {#if error}<p transition:slide={{ duration: 150 }} class="font-sans text-[13px] leading-5 text-danger" role="alert">{error}</p>{/if}
</div>
