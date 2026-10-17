<script lang="ts">
  // SCHEDULE THIS — put the agent you are already talking to on a clock,
  // without leaving the session. The job lives on the agent's own scheduler
  // (the fleet door creates the same kind), and this one records the thread
  // that is waiting on it: every run posts its answer back here as an
  // ordinary agent turn, which is why the cron needs no reader of its own.
  //
  // CREATE STAYS EXPLICIT (UI-CONVENTIONS: autosave is for pickers and
  // toggles). This one starts a timer that will spend model calls while
  // nobody is watching, which is exactly the class of action that keeps its
  // button — the form's own "Create job".
  import { Clock } from '@lucide/svelte'
  import { useQueryClient } from '@tanstack/svelte-query'
  import CronForm from '@/components/fleet/CronForm.svelte'
  import IconButton from '@/components/ui/IconButton.svelte'
  import Modal from '@/components/ui/Modal.svelte'
  import { scheduleFromConversation } from '@/lib/schedules'
  import { toastError } from '@/lib/toast.svelte'
  import SchedulesPanel from './SchedulesPanel.svelte'

  let {
    conversationId,
    canManage = null,
  }: {
    /** The session being scheduled. Null is the real "nothing selected" state
     *  and renders NOTHING: unlike drafting tickets, there is no useful
     *  disabled reading of this — a schedule needs a thread to report into
     *  and an agent to run on, and both arrive together. */
    conversationId: string | null
    /** Whether this viewer manages the session's agent, when the host knows.
     *
     *  NULL IS "NOBODY ASKED", and it shows the button. Only an agent's
     *  managers may schedule it (the route answers 403 with exactly that),
     *  and the only read that carries `canManage` is the fleet roster — a
     *  heavy payload to fetch on a work surface to decide one icon's
     *  visibility. So a host that already holds the answer passes it and the
     *  control disappears for everyone else; a host that does not lets the
     *  refusal arrive as a toast, which says the same thing in words. */
    canManage?: boolean | null
  } = $props()

  const qc = useQueryClient()
  let open = $state(false)
  let busy = $state(false)

  const create = async (input: { name: string; schedule: string; prompt: string }): Promise<boolean> => {
    const id = conversationId
    if (!id) return false
    busy = true
    try {
      await scheduleFromConversation(id, input)
      // The panel below is stale the moment the job exists.
      void qc.invalidateQueries({ queryKey: ['conversation-schedules', id] })
      return true
    } catch (e) {
      // 403 "only this agent's managers can schedule it" is the one every
      // viewer can hit, and it is news about this request rather than about
      // the form — so it goes out as a toast and the form keeps what was
      // typed into it.
      toastError('Could not schedule it', e)
      return false
    } finally {
      busy = false
    }
  }
</script>

{#if conversationId && canManage !== false}
  {@const sessionId = conversationId}
  <IconButton size="sm" title="Schedule this agent" onclick={() => (open = true)}>
    <Clock size={14} />
  </IconButton>

  {#if open}
    <!-- Mount-gated, so each open starts on a clean form rather than on
         whatever the last visit left in it. The modal portals to <body>, so
         standing it here beside the button is safe. -->
    <Modal {open} onClose={() => (open = false)} title="Schedule" width="max-w-lg">
      <div class="space-y-5">
        <p class="font-sans text-sm text-muted">
          A standing job on this agent. Every run posts its answer back into this session, so the thread is the record
          of what it found.
        </p>
        <CronForm onCreate={create} {busy} />
        <SchedulesPanel conversationId={sessionId} />
      </div>
    </Modal>
  {/if}
{/if}
