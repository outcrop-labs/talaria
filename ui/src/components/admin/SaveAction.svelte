<script lang="ts">
  import type { Snippet } from 'svelte'
  import { useExplicitSave } from '@/components/settings/autosave.svelte'
  import Button from '@/components/ui/Button.svelte'
  import { cn } from '@/lib/cn'

  // The Save beside a field that cannot save on change: a key, a password.
  // It is the gold primary while there is something to save and is not shown
  // otherwise, so an empty field never sits beside a Save that looks switched
  // off. Its cell is ahead of the field and keeps its width: the field ends
  // on the card's edge with the fields above it, and does not move when the
  // button arrives. A group of fields that saves together ends in
  // `SettingsSaveBar` instead.
  let {
    dirty,
    busy = false,
    always = false,
    size = 'sm',
    onclick,
    children,
  }: {
    dirty: boolean
    busy?: boolean
    /** A form someone opened, with a Cancel beside it: the button stays and
     *  is disabled until the form is valid, as a dialog's primary is. */
    always?: boolean
    /** `md` beside an input, so the two are one height. */
    size?: 'sm' | 'md'
    onclick: () => void
    children: Snippet
  } = $props()

  // While one is on screen the page's header cannot say every change saves
  // by itself.
  useExplicitSave()
</script>

<!-- visibility, not display: hidden it is out of the tab order and the accessibility tree, and still holds its cell. -->
<span class={cn('inline-flex shrink-0', !always && '-order-1', !always && !dirty && !busy && 'invisible')}>
  <!-- One variant while hidden and shown: an outline is 2px wider, and the field would move by that. -->
  <Button {size} variant={always && !dirty ? 'outline' : 'primary'} disabled={!dirty || busy} {onclick}>
    {@render children()}
  </Button>
</span>
