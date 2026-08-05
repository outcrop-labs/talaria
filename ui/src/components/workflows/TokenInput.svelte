<script lang="ts">
  import Chip from '@/components/ui/Chip.svelte'
  import Input from '@/components/ui/Input.svelte'

  /** Free-entry token list: type, Enter adds, ✕ removes. */
  let { value, onChange, placeholder }: { value: string[]; onChange: (v: string[]) => void; placeholder: string } = $props()

  let draft = $state('')
  const add = () => {
    const t = draft.trim()
    if (!t || value.includes(t)) {
      draft = ''
      return
    }
    onChange([...value, t])
    draft = ''
  }
</script>

<div class="flex flex-wrap items-center gap-1">
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
    class="w-40"
  />
</div>
