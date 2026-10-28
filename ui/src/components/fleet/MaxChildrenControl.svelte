<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import Input from '@/components/ui/Input.svelte'
  import { patchAgentMeta, type AgentDef } from '@/lib/fleet-defs'
  import { toastError } from '@/lib/toast.svelte'

  // How many subagents this agent may run at once. The cap is an operator's
  // decision about the box the fleet runs on, not the agent's about its own
  // work — the renderer writes it from the column every render, so a def
  // cannot author its way past it. A running Hermes reads it from the
  // rewritten config.yaml on its next turn, so a change needs no restart.
  //
  // EXPLICIT APPLY, like the toolsets beside it: a write re-renders the fleet
  // and lands on a running agent's next turn — a deploy, which UI-CONVENTIONS
  // keeps behind Apply — and a number field passes through 1 on its way to
  // 16, one PATCH and one render per spinner click. Apply appears only when the
  // field holds a valid number that differs from the saved one; an empty or
  // unparseable field is a slip, not an instruction (0 is the one value that
  // silently stops the agent delegating), so it can never be applied.
  let { def, canManage }: { def: AgentDef; canManage: boolean } = $props()

  // The server's own bounds (fleet.defs.$id PATCH reads maxChildren as an int
  // in 0..=16). Restated here so the field cannot offer what the route would
  // refuse.
  const MIN = 0
  const MAX = 16

  const qc = useQueryClient()
  let saving = $state(false)
  // svelte-ignore state_referenced_locally -- reason: a draft until Apply; the refetch after a save must not clobber a number being typed
  let draft: number | null = $state(def.maxChildren)

  /** The number Apply would send, or null when the field holds nothing it
   *  could honestly send. */
  const pending = $derived.by(() => {
    if (draft === null || draft === undefined || !Number.isFinite(Number(draft))) return null
    const n = Math.min(MAX, Math.max(MIN, Math.round(Number(draft))))
    return n === def.maxChildren ? null : n
  })

  const reset = () => {
    draft = def.maxChildren
  }

  const apply = async () => {
    const n = pending
    if (n === null) return
    saving = true
    const res = await patchAgentMeta(def.id, { maxChildren: n })
    saving = false
    if (res.error) {
      toastError('Could not change the subagent cap', res.error)
      reset()
      return
    }
    draft = n
    await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
  }
</script>

<div>
  <div class="mb-1 flex items-center gap-1.5">
    <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Subagents</span>
    <InfoTip
      text="How many helpers this agent may run at once, at most {MAX}. 0 means it delegates to nobody and does every job in its own turn. An applied change reaches the running agent on its next turn, without a restart."
    />
  </div>
  {#if canManage}
    <div class="flex items-center gap-2">
      <Input
        size="sm"
        type="number"
        min={MIN}
        max={MAX}
        step={1}
        bind:value={draft}
        disabled={saving}
        title="Subagents this agent may run at once, 0 to {MAX}"
        onkeydown={(e: KeyboardEvent) => {
          if (e.key === 'Enter') void apply()
          if (e.key === 'Escape') reset()
        }}
        class="w-20 font-[var(--font-mono)]"
      />
      {#if pending !== null}
        <Button size="xs" variant="outline" disabled={saving} onclick={() => void apply()}>
          {saving ? 'Applying' : 'Apply'}
        </Button>
        <Button size="xs" variant="ghost" disabled={saving} onclick={reset}>Reset</Button>
      {/if}
    </div>
  {:else}
    <div class="text-fg">{def.maxChildren === 0 ? '0 · delegates to nobody' : String(def.maxChildren)}</div>
  {/if}
</div>
