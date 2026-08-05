<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import { useSavedFlash } from '@/components/ui/save-button.svelte'

  // Where the org account's agents build: a Shared Drive so files are team-owned,
  // a specific calendar, and an optional send-as alias for outgoing mail.
  let { targets }: { targets: { driveFolderId: string | null; calendarId: string | null; sendAs: string | null } } = $props()

  const qc = useQueryClient()
  let drive = $state(targets.driveFolderId ?? '')
  let cal = $state(targets.calendarId ?? '')
  let sendAs = $state(targets.sendAs ?? '')
  const savedFlash = useSavedFlash()
  const dirty = $derived(drive !== (targets.driveFolderId ?? '') || cal !== (targets.calendarId ?? '') || sendAs !== (targets.sendAs ?? ''))

  const save = async () => {
    const r = await fetch('/api/integrations/google/org', {
      method: 'PUT',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ driveFolderId: drive, calendarId: cal, sendAs }),
    })
    if (r.ok) {
      await qc.invalidateQueries({ queryKey: ['org-google'] })
      savedFlash.flash()
    }
  }
</script>

<div class="mt-4 space-y-3 rounded-md border border-line p-4">
  <div class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Where agents build</div>
  <div>
    <label class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Shared Drive / folder ID</label>
    <Input size="sm" bind:value={drive} placeholder="Shared Drive or folder ID" />
    <div class="mt-1 text-[11px] text-muted">Files agents create land here (team-owned). Blank = the account’s My Drive. The org account must be a member of the Shared Drive.</div>
  </div>
  <div>
    <label class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Calendar ID</label>
    <Input size="sm" bind:value={cal} placeholder="team@group.calendar.google.com" />
    <div class="mt-1 text-[11px] text-muted">Org events land here. Blank = the account’s primary calendar.</div>
  </div>
  <div>
    <label class="mb-1.5 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Send mail as</label>
    <Input size="sm" bind:value={sendAs} placeholder="support@yourdomain.com" />
    <div class="mt-1 text-[11px] text-muted">A verified send-as alias on the org account for outgoing mail. Blank = the account’s own address.</div>
  </div>
  <div class="flex items-center gap-3">
    <Button size="sm" onclick={() => void save()} disabled={!dirty}>Save</Button>
    {#if savedFlash.saved}<span class="text-xs text-success">Saved</span>{/if}
  </div>
</div>
