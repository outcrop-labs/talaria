<script lang="ts">
  import { useQueryClient } from '@tanstack/svelte-query'
  import Button from '@/components/ui/Button.svelte'
  import Checkbox from '@/components/ui/Checkbox.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import { patchAgentMeta, useFleetDefs, type AgentDef } from '@/lib/fleet-defs'
  import { toastError } from '@/lib/toast.svelte'
  import { normalizeToolsets, sameToolsets, toggleToolset, toolsetRows } from './toolsets'

  /** THE HERMES TOOLSETS THIS AGENT DOES NOT GET. The stored list is the
   *  absence (`disabledToolsets`); a checked box is the toolset being ON, and
   *  `toolsets.ts` owns that conversion so the sign lives in one place.
   *
   *  The vocabulary comes from the server (`/api/fleet/defs` → `toolsets`)
   *  rather than a list restated here: a name Talaria does not govern reaches
   *  the rendered config and Hermes ignores it in silence, which would show an
   *  operator a toolset they believe is closed and an agent that still has it.
   *  No `toolsets` on the read (an older api) means no grid at all — better
   *  than an empty one that reads as "this agent has nothing".
   *
   *  This is the one control on the MCP tab that does NOT autosave: a change
   *  rolls the container, so per UI-CONVENTIONS it waits for an explicit
   *  Apply. */
  let { def, canManage }: { def: AgentDef; canManage: boolean } = $props()

  const qc = useQueryClient()
  // Already in flight for the roster that opened this modal, so this is a
  // cache read, not a second fetch.
  const defsQuery = useFleetDefs(() => true)
  const toolsets = $derived(defsQuery.data?.toolsets)

  // null = untouched, showing the def. A list = the operator's pending edit.
  let pending = $state<string[] | null>(null)
  let saving = $state(false)
  const shown = $derived(pending ?? def.disabledToolsets)
  const rows = $derived(toolsets ? toolsetRows(toolsets.governed, toolsets.mandatoryDisabled, shown) : [])
  // Compared against the def's list in its STORABLE spelling, because Apply
  // must not offer to roll a container for a list that already means what is
  // stored: order is not a change, and neither is a legacy def carrying one of
  // the two names the renderer closes anyway.
  const stored = $derived(normalizeToolsets(def.disabledToolsets, toolsets?.mandatoryDisabled))
  const dirty = $derived(pending !== null && !sameToolsets(pending, stored))

  const set = (name: string, on: boolean) => {
    pending = toggleToolset(shown, name, on, toolsets?.mandatoryDisabled)
  }

  const apply = async () => {
    if (!pending) return
    saving = true
    const res = await patchAgentMeta(def.id, { disabledToolsets: pending })
    saving = false
    if (res.error) {
      // The pending list stays, so the operator can retry or reset rather
      // than re-ticking ten boxes.
      toastError('Could not change this agent’s toolsets', res.error)
      return
    }
    pending = null
    await qc.invalidateQueries({ queryKey: ['fleet-defs'] })
  }
</script>

{#if rows.length}
  <div>
    <div class="flex items-center gap-2">
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Hermes toolsets</span>
      <InfoTip
        text="The tools the agent's own harness carries, before any MCP server. A box here is the toolset being ON for this agent; clearing one closes it on the next roll. Talaria's own todo and clarify tools replace Hermes' fleet-wide, so those two stay closed."
      />
      <span class="flex-1"></span>
      {#if canManage && dirty}
        <Button size="sm" variant="ghost" disabled={saving} onclick={() => (pending = null)}>Reset</Button>
        <Button size="sm" disabled={saving} onclick={() => void apply()}>Apply</Button>
      {/if}
    </div>
    <div class="mt-1 font-sans text-xs text-muted">
      Off takes effect on the next roll; browser off also skips the browser setup.
    </div>
    <div class="mt-2 grid grid-cols-2 gap-x-4 gap-y-1.5 sm:grid-cols-3">
      {#each rows as r (r.name)}
        <Checkbox
          checked={r.on}
          disabled={!canManage || r.forced || saving}
          onChange={(on) => set(r.name, on)}
          label={r.name}
          title={r.forced
            ? `Talaria's own ${r.name} tool is the one a person sees, so Hermes' stays closed for every agent`
            : r.on
              ? `${r.name} is on for this agent`
              : `${r.name} is closed for this agent`}
          class={r.on ? 'text-fg' : undefined}
        />
      {/each}
    </div>
  </div>
{/if}
