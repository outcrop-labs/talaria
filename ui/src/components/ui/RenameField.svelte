<script lang="ts">
  import Input from './Input.svelte'

  // A name you edit in place: Enter (or leaving the field) commits, and the
  // field remounts when the name it is editing changes under it, so a refetch
  // can never leave stale text sitting in the box. Owns no write — the caller
  // gets the trimmed, non-empty, actually-changed value and decides what to do
  // with it.
  let {
    value,
    onCommit,
    key,
    disabled = false,
    class: className,
  }: {
    /** The current name. */
    value: string
    /** Called with the trimmed name, only when it is non-empty and differs. */
    onCommit: (next: string) => void
    /** Identity of the thing being renamed — a change remounts the field. */
    key: string
    disabled?: boolean
    class?: string
  } = $props()
</script>

{#key key}
  <Input
    size="sm"
    {value}
    {disabled}
    onblur={(e) => {
      const v = e.currentTarget.value.trim()
      if (v && v !== value) onCommit(v)
    }}
    onkeydown={(e) => e.key === 'Enter' && e.currentTarget.blur()}
    class={className}
  />
{/key}