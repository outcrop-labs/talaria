<script lang="ts">
  import { mount, unmount } from 'svelte'
  import { cn } from '@/lib/cn'
  import AgentMediaImage from '@/components/artifacts/AgentMediaImage.svelte'
  import { renderMarkdown } from './markdown'

  // Full markdown for chat — the unified pipeline + Mercury element styling
  // live in markdown.ts; this component renders the result and wires up the
  // two behaviors static HTML can't carry: the code-block copy button
  // (delegated click) and the agent-media save affordance (mounted component).
  // Reuse — do not re-render markdown inline.
  let { children, class: className }: { children: string; class?: string } = $props()

  const html = $derived(renderMarkdown(children))

  let container = $state<HTMLDivElement | null>(null)

  // Agent-produced images (served out of an agent container) get the
  // save-to-artifacts affordance: the pipeline leaves a placeholder span that
  // we hydrate with a real <AgentMediaImage> after every render.
  $effect(() => {
    void html // re-run after {@html} has replaced the DOM
    if (!container) return
    const instances: Record<string, unknown>[] = []
    for (const slot of container.querySelectorAll<HTMLElement>('[data-agent-media]')) {
      instances.push(mount(AgentMediaImage, { target: slot, props: { src: slot.dataset.src ?? '', alt: slot.dataset.alt ?? '' } }))
    }
    return () => {
      for (const i of instances) unmount(i)
    }
  })

  // Copy button on fenced code (chrome emitted by markdown.ts, same markup as
  // CodeBlock.svelte). Delegated because the markup arrives via {@html}; the
  // source text is the highlighted element's textContent, minus the trailing
  // newline fenced blocks carry.
  function oncopy(e: MouseEvent) {
    const btn = (e.target as HTMLElement).closest<HTMLButtonElement>('button[data-copy-code]')
    if (!btn || !container?.contains(btn)) return
    const code = btn.closest('[data-code-block]')?.querySelector('pre code')?.textContent ?? ''
    navigator.clipboard
      .writeText(code.replace(/\n$/, ''))
      .then(() => {
        btn.textContent = 'copied ✓'
        setTimeout(() => {
          btn.textContent = 'copy'
        }, 1500)
      })
      .catch(() => {
        /* clipboard unavailable */
      })
  }
</script>

<!-- Rendered prose is a READING surface — sans (code re-asserts mono). -->
<!-- svelte-ignore a11y_no_static_element_interactions a11y_click_events_have_key_events -->
<div bind:this={container} class={cn('select-text space-y-2 break-words font-sans', className)} onclick={oncopy}>
  {@html html}
</div>
