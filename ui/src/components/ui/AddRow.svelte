<script lang="ts">
  import Button from './Button.svelte'
  import Input from './Input.svelte'

  // The trailing "type a name, press Add" row under a registry — the board's
  // statuses and its labels both end in one, and it was written twice.
  //
  // It owns the draft and nothing else: the caller says what a new record is
  // called, and gets the trimmed, non-empty name back. The write, its error
  // surface and the refetch stay with the caller, which is why this does not
  // reach for a query client: one registry re-reads three keys through a `run`
  // helper, the other two through `refresh`, and neither belongs here.
  //
  // NOT `InlineCreate` (the `+` that expands): that is a click-to-reveal
  // affordance, this row is always open and names its action, and swapping
  // one for the other would redesign both surfaces.
  let {
    placeholder,
    onSubmit,
  }: {
    placeholder: string
    /** Called with the trimmed name. Return the write's promise — or the write
     *  chained onto its refresh — and the field clears once that settles;
     *  return nothing and it clears at once. */
    onSubmit: (value: string) => void | Promise<unknown>
  } = $props()

  let value = $state('')

  const submit = async () => {
    const v = value.trim()
    if (!v) return
    await onSubmit(v)
    value = ''
  }
</script>

<div class="flex gap-2">
  <Input size="sm" bind:value {placeholder} onkeydown={(e) => e.key === 'Enter' && void submit()} class="flex-1" />
  <Button size="sm" disabled={!value.trim()} onclick={() => void submit()}>Add</Button>
</div>