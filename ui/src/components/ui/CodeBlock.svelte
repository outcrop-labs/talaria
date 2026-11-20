<script lang="ts">
  import type { Snippet } from 'svelte'
  import { cn } from '@/lib/cn'
  import { focusGold } from '@/components/chat/chat-chrome'
  import { useUiVoice } from '@/lib/ui-voice'

  // Fenced code block with a language label + copy button. Highlighting is applied
  // by rehype-highlight (highlight.js); colours come from the Mercury hljs theme in
  // styles.css. Reuse this anywhere code is shown. (The Markdown pipeline emits
  // this same chrome as static HTML — see markdown.ts — keep the two in sync.)
  let {
    code,
    language,
    class: className,
    children,
  }: {
    code: string
    language?: string
    class?: string
    children?: Snippet
  } = $props()

  let copied = $state(false)

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(code)
      copied = true
      setTimeout(() => (copied = false), 1500)
    } catch {
      /* clipboard unavailable */
    }
  }

  const sans = useUiVoice() === 'sans'
</script>

<!-- Ground inset well (spec §8): --code-bg + hairline, control radius 6. -->
<div class={cn('group my-3 overflow-hidden rounded-md border border-line bg-[var(--code-bg)]', className)}>
  <!-- Section-header row: 10px mono uppercase ink-dim label + right-aligned ghost action. -->
  <div class="flex items-center justify-between border-b border-line-subtle px-3 py-1.5">
    <span class={sans ? 'font-mono text-xs leading-4 text-muted' : 'font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim'}>{language || 'text'}</span><!-- voice-ok: the mono branch, as it was -->
    <button
      type="button"
      onclick={copy}
      class={cn(
        sans
          ? 'font-sans text-xs font-medium leading-4 text-muted transition-colors hover:text-fg'
          : 'font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-fg', // voice-ok: the mono branch, as it was
        focusGold,
      )}
    >
      {#if sans}{copied ? 'Copied' : 'Copy'}{:else}{copied ? 'copied ✓' : 'copy'}{/if}
    </button>
  </div>
  <pre class="overflow-x-auto px-4 py-3 font-mono text-[0.85rem] leading-relaxed">{#if children}{@render children()}{:else}<code>{code}</code>{/if}</pre>
</div>
