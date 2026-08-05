<script lang="ts">
  import Chip from '@/components/ui/Chip.svelte'
  import Input from '@/components/ui/Input.svelte'

  /** Free-entry token row (labels / keywords) with friendlier affordances. */
  let { value, onChange, placeholder }: { value: string[]; onChange: (v: string[]) => void; placeholder: string } = $props()

  let draft = $state('')
  const add = () => {
    const t = draft.trim()
    draft = ''
    if (t && !value.includes(t)) onChange([...value, t])
  }
</script>

<div class="flex flex-wrap items-center gap-1.5">
  {#each value as t (t)}
    <Chip onRemove={() => onChange(value.filter((x) => x !== t))}>
      {t}
    </Chip>
  {/each}
  <Input
    size="sm"
    bind:value={draft}
    onkeydown={(e) => {
      if (e.key === 'Enter') {
        e.preventDefault()
        add()
      }
    }}
    onblur={add}
    {placeholder}
    class="w-44"
  />
</div>
