<script lang="ts">
  import { Send } from '@lucide/svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import Button from '@/components/ui/Button.svelte'

  let { onClose }: { onClose: () => void } = $props()

  let to = $state('')
  let subject = $state('')
  let body = $state('')
  let busy = $state(false)
  let status = $state<string | null>(null)

  const send = async () => {
    if (!to.trim()) return
    busy = true
    status = null
    try {
      const r = await fetch('/api/integrations/google/gmail/send', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ to: to.trim(), subject, body }),
      })
      const j = (await r.json().catch(() => null)) as { sent?: unknown; message?: string } | null
      if (r.ok && j?.sent) onClose()
      else status = j?.message ?? 'Could not send the email.'
    } finally {
      busy = false
    }
  }
</script>

{#snippet title()}
  <span class="flex items-center gap-2"><Send size={15} class="text-muted" /> New message</span>
{/snippet}

<!-- The shared Modal (Escape + backdrop close) with the shared field
     primitives — this was the one dialog still hand-rolling both. -->
<Modal open {onClose} {title} width="max-w-lg">
  <div class="space-y-2">
    <Input autofocus bind:value={to} placeholder="To" />
    <Input bind:value={subject} placeholder="Subject" />
    <Textarea bind:value={body} placeholder="Write your message" rows={8} class="w-full resize-y" />
    <div class="flex items-center gap-3">
      <Button size="sm" onclick={() => void send()} disabled={busy || !to.trim()}>
        <Send size={13} class="mr-1" /> {busy ? 'Sending' : 'Send'}
      </Button>
      {#if status}<span class="font-sans text-xs text-danger">{status}</span>{/if}
    </div>
  </div>
</Modal>
