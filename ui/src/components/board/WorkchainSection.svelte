<script lang="ts">
  // TaskDetail's workchain section (TALA-30 step 5): where this ticket sits
  // in its chain, with the editor's verbs — reorder within the chain, leave
  // the chain. Joining a chain lives in the Workchains view's pickers (the
  // rail's + and the Unchained rows); here the chain is context, not the
  // show. Step-assignee changes are deliberately ABSENT: one chain per task
  // is v1, and reassigning a step is a task edit, not a chain edit.
  import { ArrowLeft, ArrowRight, Link2Off } from '@lucide/svelte'
  import Section from './Section.svelte'
  import { toastError } from '@/lib/toast.svelte'
  import { removeWorkchainStep, updateWorkchain } from '@/lib/workchain-client'
  import { chainIsLinear, moveStepOrder, type Workchain, type WorkchainStep } from '@/lib/workchain-rules'

  let {
    chain,
    step,
    canEdit,
    onChanged,
  }: {
    /** The chain holding this ticket, if any. */
    chain: Workchain | null
    /** The ticket's own step in it. */
    step: WorkchainStep | null
    canEdit: boolean
    /** A write landed; the owner re-reads (['board-workchains']). */
    onChanged: () => void
  } = $props()

  const failure = (what: string) => (e: unknown) =>
    toastError(`${what} failed`, e)

  /** The move verbs ship the linear `positions` payload, and the api rewrites
   *  the chain's edges to ONE line through it. On a branched chain that is a
   *  silent flattening — a nudge here would erase every fan-out and join the
   *  canvas drew. So they only appear on a chain that is already a line;
   *  a branched chain is reordered by moving its wires. */
  const linear = $derived(chain ? chainIsLinear(chain) : false)

  const move = (delta: -1 | 1) => {
    if (!chain || !step) return
    const order = moveStepOrder(chain.steps.map((s) => s.taskId), step.taskId, delta)
    if (!order) return
    void updateWorkchain(chain.id, { positions: order.map((taskId, position) => ({ taskId, position })) })
      .then(onChanged)
      .catch(failure('Reordering the chain'))
  }

  const leave = () => {
    if (!chain || !step) return
    void removeWorkchainStep(chain.id, step.taskId).then(onChanged).catch(failure('Leaving the chain'))
  }
</script>

{#if chain && step}
  <Section label="Workchain">
    <div class="flex items-center gap-2 rounded-lg border border-line bg-panel px-3 py-2">
      <span class="min-w-0 flex-1 truncate font-sans text-sm font-medium text-fg">{chain.name}</span>
      {#if chain.paused}
        <span class="font-mono text-[10px] uppercase tracking-[0.05em] text-warning">paused</span>
      {/if}
      <span class="font-mono text-[10px] tracking-[0.05em] text-muted">
        step {step.position + 1} of {chain.steps.length}
      </span>
      {#if canEdit}
        <span class="flex items-center gap-0.5">
          {#if linear}
            <button
              type="button"
              title="Move earlier in the chain"
              disabled={step.position === 0}
              class="rounded p-1 text-muted transition-colors hover:text-fg disabled:opacity-30"
              onclick={() => move(-1)}
            >
              <ArrowLeft size={13} />
            </button>
            <button
              type="button"
              title="Move later in the chain"
              disabled={step.position === chain.steps.length - 1}
              class="rounded p-1 text-muted transition-colors hover:text-fg disabled:opacity-30"
              onclick={() => move(1)}
            >
              <ArrowRight size={13} />
            </button>
          {:else}
            <span
              class="font-mono text-[9px] uppercase tracking-[0.05em] text-muted"
              title="This chain branches — its order is its wiring. Rewire it on the workchains canvas."
            >
              branched
            </span>
          {/if}
          <button
            type="button"
            title="Leave the chain (the ticket stays)"
            class="rounded p-1 text-muted transition-colors hover:text-danger"
            onclick={() => leave()}
          >
            <Link2Off size={13} />
          </button>
        </span>
      {/if}
    </div>
  </Section>
{/if}
