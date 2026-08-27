<script lang="ts">
  import { Send } from '@lucide/svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import Button from '@/components/ui/Button.svelte'
  import { errorMessage, postJsonOr } from '@/lib/fetch-json'

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
      // 409/502 carry the route's human sentence ("Connect a Google account
      // first.") for this inline status line; anything else rejects below.
      const j = await postJsonOr<{ sent?: { id: string; threadId: string }; message?: string }>(
        '/api/integrations/google/gmail/send',
        { to: to.trim(), subject, body },
        [409, 502],
      )
      if (j.sent) onClose()
      else status = j.message ?? 'Could not send the email.'
    } catch (e) {
      status = errorMessage(e)
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
