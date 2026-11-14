<script lang="ts">
  import type { Snippet } from 'svelte'
  import { useExplicitSave } from '@/components/settings/autosave.svelte'
  import Button from '@/components/ui/Button.svelte'

  // The Save beside a field that cannot save on change: a key, a password.
  // It is the gold primary only while there is something to save, and a
  // disabled outline otherwise, so a clean field shows no call to action. A
  // group of fields that saves together ends in `SettingsSaveBar` instead.
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

  // While one is on screen the page's header cannot say every change saves
  // by itself.
  useExplicitSave()
</script>

<Button {size} variant={dirty ? 'primary' : 'outline'} disabled={!dirty || busy} {onclick}>
  {@render children()}
</Button>
