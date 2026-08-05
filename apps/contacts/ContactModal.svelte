<script lang="ts">
  import { alert, Button, confirm, DangerLink, Input, Modal, Select, Textarea } from '@talaria/sdk'
  import { api, type Contact, type ContactDoc } from './contacts'

  let {
    doc,
    stages,
    onClose,
    onSaved,
  }: { doc: ContactDoc | null; stages: string[]; onClose: () => void; onSaved: () => void } = $props()

  // Clone the doc's data — the inputs bind straight into `form`, and writing
  // through a $state proxy of the cached query object would edit the list
  // behind the modal before (or without) a save.
  let form = $state<Contact>(doc ? { ...doc.data } : { name: '', stage: stages[0] })
  let busy = $state(false)

  const save = async () => {
    if (!form.name?.trim()) return
    busy = true
    try {
      if (doc) await api.patch(`contacts/${doc.id}`, form)
      else await api.post('contacts', form)
      onSaved()
    } catch (e) {
      void alert({ title: 'Could not save', message: (e as Error).message })
    } finally {
      busy = false
    }
  }

  const remove = async () => {
    if (!doc) return
    if (!(await confirm({ title: `Delete ${doc.data.name}?`, message: 'This cannot be undone.', danger: true }))) return
    busy = true
    try {
      await api.del(`contacts/${doc.id}`)
      onSaved()
    } finally {
      busy = false
    }
  }
</script>

<Modal open {onClose} title={doc ? doc.data.name : 'New contact'}>
  <div class="space-y-3">
    <Input bind:value={form.name} placeholder="Name" autofocus />
    <div class="flex gap-2">
      <Input bind:value={form.company} placeholder="Company" class="flex-1" />
      <Input bind:value={form.email} placeholder="Email" class="flex-1" />
    </div>
    <Select bind:value={form.stage}>
      {#each stages as s (s)}
        <option value={s}>{s}</option>
      {/each}
    </Select>
    <Textarea bind:value={form.notes} placeholder="Notes" rows={4} />
    <div class="flex items-center justify-between pt-1">
      {#if doc}
        <!-- Destructive trigger: quiet mono link → safety orange, paired with
             the confirm() step — never an orange fill (spec §8). -->
        <DangerLink disabled={busy} onClick={() => void remove()}>Delete</DangerLink>
      {:else}
        <span></span>
      {/if}
      <div class="flex gap-2">
        <Button variant="outline" onclick={onClose}>Cancel</Button>
        <Button disabled={busy || !form.name?.trim()} onclick={() => void save()}>
          {doc ? 'Save' : 'Create'}
        </Button>
      </div>
    </div>
  </div>
</Modal>
