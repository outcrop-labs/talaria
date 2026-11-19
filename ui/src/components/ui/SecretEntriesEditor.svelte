<script lang="ts" module>
  /** One line of a secret document: a handle the agent substitutes by, a human
   *  label, and the value itself. */
  export interface SecretEntry {
    key: string
    label: string
    value: string
  }

  /** A fresh, empty row. Exported so callers reset with the same shape the
   *  editor adds — a caller inventing `{ key: '', label: '', value: '' }` by
   *  hand is how the two drift apart again. */
  export const emptySecretEntry = (): SecretEntry => ({ key: '', label: '', value: '' })
</script>

<script lang="ts">
  import { Plus, X } from '@lucide/svelte'
  import { useUiVoice } from '@/lib/ui-voice'
  import Button from './Button.svelte'
  import Input from './Input.svelte'

  // THE ENTRIES OF A SECRET, edited in place.
  //
  // This existed twice, near-verbatim: once in the personal vault and once in
  // the workspace panel. Same grid, same password field, same remove button
  // disabled at one row, same "Another entry". Two copies of a form that
  // handles credentials is two places to get the password field wrong, and the
  // copies had already drifted in their tooltips and placeholders.
  //
  // ONE ENTRY OR MANY, and the form does not ask you to decide first: a
  // document with one entry IS a single secret. That is why there is always at
  // least one row and why removing the last one is disabled rather than hidden
  // — an empty editor would imply a secret with nothing in it is a thing you
  // could save.
  //
  // The value field is `type="password"` with autocomplete off, and it is the
  // reason this is one component: it must not be possible to add a third
  // caller that spells that differently.
  let {
    entries = $bindable(),
    keyPlaceholder = 'secret_key',
    labelPlaceholder = 'Secret key',
    valuePlaceholder = 'value (stored sealed, never shown again)',
  }: {
    entries: SecretEntry[]
    keyPlaceholder?: string
    labelPlaceholder?: string
    valuePlaceholder?: string
  } = $props()

  // In the settings voice the caller's field label names the rows, so the
  // caption is not drawn.
  const sans = useUiVoice() === 'sans'
</script>

{#if sans}
  <div class="space-y-2">
    {#each entries as e, i (i)}
      <div class="grid gap-2 sm:grid-cols-[1fr_1fr_2fr_auto]">
        <Input bind:value={e.key} placeholder={keyPlaceholder} aria-label="Handle" size="sm" class="font-mono" />
        <Input bind:value={e.label} placeholder={labelPlaceholder} aria-label="Label" size="sm" />
        <Input
          bind:value={e.value}
          type="password"
          autocomplete="off"
          placeholder={valuePlaceholder}
          aria-label="Value"
          size="sm"
        />
        <Button
          size="sm"
          variant="ghost"
          class="px-2"
          disabled={entries.length === 1}
          onclick={() => (entries = entries.filter((_, n) => n !== i))}
          title="Remove this entry"
          aria-label="Remove this entry"
        >
          <X size={16} aria-hidden="true" />
        </Button>
      </div>
    {/each}
    <Button size="sm" variant="ghost" class="-ml-3" onclick={() => (entries = [...entries, emptySecretEntry()])}>
      <Plus size={16} aria-hidden="true" />
      Add entry
    </Button>
  </div>
{:else}
<p class="mt-3 font-mono text-[10px] text-ink-dim">entries</p>
{#each entries as e, i (i)}
  <div class="mt-1 grid gap-1 sm:grid-cols-[1fr_1fr_2fr_auto]">
    <Input bind:value={e.key} placeholder={keyPlaceholder} aria-label="Handle" size="sm" class="font-[var(--font-mono)]" />
    <Input bind:value={e.label} placeholder={labelPlaceholder} aria-label="Label" size="sm" />
    <Input
      bind:value={e.value}
      type="password"
      autocomplete="off"
      placeholder={valuePlaceholder}
      aria-label="Value"
      size="sm"
      class="font-[var(--font-mono)]"
    />
    <Button
      size="sm"
      variant="ghost"
      disabled={entries.length === 1}
      onclick={() => (entries = entries.filter((_, n) => n !== i))}
      title="Remove this entry"
      aria-label="Remove this entry"
    >
      <X size={13} aria-hidden="true" />
    </Button>
  </div>
{/each}
<Button size="sm" variant="ghost" class="mt-1" onclick={() => (entries = [...entries, emptySecretEntry()])}>
  <Plus size={13} aria-hidden="true" />
  Another entry
</Button>
{/if}
