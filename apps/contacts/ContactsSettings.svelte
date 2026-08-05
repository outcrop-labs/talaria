<script lang="ts">
  import { alert, Button, Input, Panel, SectionHeader } from '@talaria/sdk'
  import { api, useStages } from './contacts'

  // ── Settings surface ───────────────────────────────────────────────────────
  const cfg = useStages()
  let draft = $state<string | null>(null)
  let busy = $state(false)
  const value = $derived(draft ?? (cfg.data?.stages ?? []).join(', '))

  const save = async () => {
    busy = true
    try {
      await api.put('config', { stages: value.split(',').map((s) => s.trim().toLowerCase()).filter(Boolean) })
      draft = null
      await cfg.refetch()
    } catch (e) {
      void alert({ title: 'Could not save stages', message: (e as Error).message })
    } finally {
      busy = false
    }
  }
</script>

<Panel as="section">
  <SectionHeader title="Pipeline stages" class="mb-1" />
  <p class="mb-3 text-xs text-muted">Comma-separated, in order. Shared by everyone using Contacts.</p>
  <div class="flex gap-2">
    <Input {value} oninput={(e) => (draft = e.currentTarget.value)} class="flex-1" />
    <Button disabled={busy || draft === null} onclick={() => void save()}>Save</Button>
  </div>
</Panel>
