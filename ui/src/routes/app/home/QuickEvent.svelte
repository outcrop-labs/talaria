<script lang="ts">
  import Input from '@/components/ui/Input.svelte'
  import Button from '@/components/ui/Button.svelte'
  import { errorMessage, postJsonOr } from '@/lib/fetch-json'

  // Minimal create form: title + start; end defaults to +1h.
  let { onDone }: { onDone: () => void } = $props()

  let summary = $state('')
  let start = $state('')
  let busy = $state(false)
  let err = $state<string | null>(null)

  const submit = async () => {
    if (!summary.trim() || !start) return
    busy = true
    err = null
    try {
      const startISO = new Date(start).toISOString()
      const endISO = new Date(new Date(start).getTime() + 60 * 60_000).toISOString()
      // 409/502 carry the route's human sentence ("Connect a Google account
      // first.") for this inline err line; anything else rejects below.
      const j = await postJsonOr<{ event?: unknown; message?: string }>(
        '/api/integrations/google/calendar/events',
        { summary: summary.trim(), start: startISO, end: endISO },
        [409, 502],
      )
      if (j.event) onDone()
      else err = j.message ?? 'Could not create the event.'
    } catch (e) {
      err = errorMessage(e)
    } finally {
      busy = false
    }
  }
</script>

<div class="mb-3 flex flex-wrap items-center gap-2 rounded-md border border-line p-2">
  <Input
    size="sm"
    bind:value={summary}
    placeholder="Event title"
    class="min-w-0 flex-1"
  />
  <Input
    size="sm"
    type="datetime-local"
    bind:value={start}
    class="w-auto"
  />
  <Button size="sm" onclick={() => void submit()} disabled={busy || !summary.trim() || !start}>
    Add
  </Button>
  {#if err}<span class="w-full font-sans text-[11px] text-danger">{err}</span>{/if}
</div>
