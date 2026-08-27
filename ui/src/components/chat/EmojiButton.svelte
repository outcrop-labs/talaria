<script lang="ts">
  import { Smile } from '@lucide/svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import EmojiPicker from '@/components/ui/EmojiPicker.svelte'

  // The composer's smiley tile — a thin trigger over the ONE emoji picker
  // (ui/EmojiPicker: the full ~1900-glyph searchable set over the §7 shell,
  // dataset lazy-loaded on first open). There used to be a second, hand-rolled
  // ~100-glyph grid here; one picker, one dataset. Shortcodes (:smile:) still
  // work straight from the keyboard (emoji.svelte.ts).
  let { onPick, disabled }: { onPick: (ch: string) => void; disabled?: boolean } = $props()
</script>

<EmojiPicker onPick={onPick} up>
  {#snippet trigger()}
    <IconButton title="Add emoji (or type :shortcode:)" size="tile" {disabled}>
      <Smile size={15} />
    </IconButton>
  {/snippet}
</EmojiPicker>
