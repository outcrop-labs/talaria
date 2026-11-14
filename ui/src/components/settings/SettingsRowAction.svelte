<script lang="ts">
  import type { Snippet } from 'svelte'
  import Button from '@/components/ui/Button.svelte'
  import { buttonClasses } from '@/components/ui/button'
  import { useUiVoice } from '@/lib/ui-voice'

  // The one action a row shows when its state needs a person: a small
  // secondary button named for what it does ("Connect", "Reconnect",
  // "Verify"), never "Fix". The row's status says what is wrong. On a row that
  // opens it takes the chevron's place, and the row still opens when clicked.
  // `SettingsRow` draws it at the row's edge and hides the kebab of a row that
  // holds one until the row is hovered or focused.
  let {
    onclick,
    href,
    label,
    title,
    disabled = false,
    class: className,
    children,
  }: {
    onclick?: () => void
    /** The action is going somewhere: a sign-in redirect, the page that holds the setting. */
    href?: string
    /** The accessible name, when the word alone would not say what it acts on ("Connect Notion"). */
    label?: string
    title?: string
    disabled?: boolean
    class?: string
    /** The action's name, as the row's menu or dialog calls it. */
    children: Snippet
  } = $props()

  const voice = useUiVoice()
</script>

{#if href}
  <a data-row-action {href} {title} aria-label={label} class={buttonClasses({ size: 'sm', variant: 'outline', voice, className })}>{@render children()}</a>
{:else}
  <Button data-row-action size="sm" variant="outline" {title} {disabled} aria-label={label} class={className} {onclick}>{@render children()}</Button>
{/if}
