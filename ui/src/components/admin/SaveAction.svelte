<script lang="ts">
  import type { Snippet } from 'svelte'
  import Button from '@/components/ui/Button.svelte'

  // The Save of a card that does not save on change. It is the gold primary
  // only while there is something to save, and a disabled outline otherwise,
  // so a clean form shows no call to action.
  let {
    dirty,
    busy = false,
    size = 'sm',
    onclick,
    children,
  }: {
    dirty: boolean
    busy?: boolean
    /** `md` beside an input, so the two are one height. */
    size?: 'sm' | 'md'
    onclick: () => void
    children: Snippet
  } = $props()
</script>

<Button {size} variant={dirty ? 'primary' : 'outline'} disabled={!dirty || busy} {onclick}>
  {@render children()}
</Button>
