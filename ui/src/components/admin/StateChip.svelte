<script lang="ts">
  import Chip from '@/components/ui/Chip.svelte'
  import { STATE_COPY, type SecretRow } from '@/lib/secrets'

  const STATE_TONE = {
    ok: 'success',
    unreadable: 'danger',
    missing: 'neutral',
    env: 'neutral',
  } as const

  let { row }: { row: SecretRow } = $props()

  const copy = $derived(STATE_COPY[row.state])
</script>

<Chip tone={STATE_TONE[row.state]} title={copy.hint}>
  {copy.label}
</Chip>
