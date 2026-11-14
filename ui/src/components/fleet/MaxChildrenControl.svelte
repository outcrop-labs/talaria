<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import { useAutosave } from '@/components/settings/autosave.svelte'
  import SettingsRow from '@/components/settings/SettingsRow.svelte'
  import Button from '@/components/ui/Button.svelte'
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
  const autosave = useAutosave()
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
    autosave.flash()
    await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
  }
</script>

<SettingsRow
  label="Subagents"
  hint={`How many helpers it may run at once, from ${MIN} to ${MAX}.`}
  info="0 means it delegates to nobody and does every job in its own turn. An applied change reaches the running agent on its next turn, without a restart."
  saved={autosave.saved()}
>
  {#if canManage}
    {#if pending !== null}
      <Button size="sm" variant="ghost" disabled={saving} onclick={reset}>Reset</Button>
      <Button size="sm" disabled={saving} onclick={() => void apply()}>
        {saving ? 'Applying' : 'Apply'}
      </Button>
    {/if}
    <Input
      size="sm"
      type="number"
      min={MIN}
      max={MAX}
      step={1}
      bind:value={draft}
      disabled={saving}
      aria-label="Subagents"
      onkeydown={(e: KeyboardEvent) => {
        if (e.key === 'Enter') void apply()
        if (e.key === 'Escape') reset()
      }}
      class="w-20 tabular-nums"
    />
  {:else}
    <span class="font-sans text-[13px] leading-5 text-fg tabular-nums">{def.maxChildren === 0 ? '0, delegates to nobody' : String(def.maxChildren)}</span>
  {/if}
</SettingsRow>
