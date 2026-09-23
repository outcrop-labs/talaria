<script lang="ts">
  import { Check, Wrench, X } from '@lucide/svelte'
  import PlatformLinkChip from './PlatformLinkChip.svelte'
  import Button from '@/components/ui/Button.svelte'
  import { focusGold } from '@/components/chat/chat-chrome'
  import { cn } from '@/lib/cn'
  import {
    chipsBesideProse,
    decideChip,
    invokeText,
    toolDescription,
    toolFields,
    toolLabel,
    type ChatChip,
    type ChipEntity,
  } from '@/lib/chips'

  // Persisted chips that are not already painted from the prose: exposed
  // tools, protected-action approvals, unlocks, and links the agent produced
  // without pasting the URL. Clicking a tool (or an unlocked tool name)
  // opens its inputs inline. Approve/deny stay on the approval chip.
  let {
    chips,
    content = '',
    onInvoke,
    onDecided,
  }: {
    chips?: ChatChip[]
    content?: string
    onInvoke?: (text: string) => void
    onDecided?: () => void
  } = $props()

  const shown = $derived(chipsBesideProse(chips, content))
  let openTool = $state<string | null>(null)
  let draft = $state<Record<string, string>>({})
  let busy = $state<string | null>(null)
  let error = $state<string | null>(null)

  function open(chip: ChatChip) {
    const name = chip.tool ?? chip.tools?.[0] ?? ''
    if (!name) return
    openTool = openTool === chip.id ? null : chip.id
    const fields = chip.fields?.length ? chip.fields : toolFields(name)
    const next: Record<string, string> = {}
    for (const field of fields) {
      const carried = chip.inputs?.[field.name]
      next[field.name] = carried == null ? '' : String(carried)
    }
    if (!fields.length && chip.inputs) {
      for (const [key, value] of Object.entries(chip.inputs)) next[key] = value == null ? '' : String(value)
    }
    draft = next
    error = null
  }

  function fieldsFor(chip: ChatChip, name: string) {
    if (chip.fields?.length) return chip.fields
    const known = toolFields(name)
    if (known.length) return known
    return Object.keys(draft).map((name) => ({ name, label: name, type: 'string' as const }))
  }

  async function decide(chip: ChatChip, decision: 'approve' | 'deny') {
    if (!chip.actionId || busy) return
    busy = chip.actionId
    error = null
    const result = await decideChip(chip.actionId, decision)
    busy = null
    if (!result.ok) {
      error = result.error ?? 'That did not go through.'
      return
    }
    onDecided?.()
  }

  function run(chip: ChatChip) {
    const name = chip.tool ?? ''
    if (!name || !onInvoke) return
    onInvoke(invokeText(name, draft))
    openTool = null
  }
</script>

{#if shown.length}
  <div class="mt-2 flex flex-wrap items-start gap-1.5">
    {#each shown as chip (chip.id)}
      {#if chip.kind === 'link' && chip.href && chip.entity}
        <PlatformLinkChip href={chip.href} entity={chip.entity as ChipEntity} title={chip.title} />
      {:else if chip.kind === 'tool'}
        {@const name = chip.tool ?? ''}
        <button
          type="button"
          class={cn(
            'inline-flex max-w-full items-center gap-1.5 rounded-md border border-line bg-raised px-2 py-1 font-sans text-xs text-fg transition-colors hover:border-line-strong hover:bg-hover',
            focusGold,
            openTool === chip.id && 'border-line-strong',
          )}
          aria-expanded={openTool === chip.id}
          title={chip.description || toolDescription(name)}
          onclick={() => open(chip)}
        >
          <Wrench size={13} class="shrink-0 text-muted" />
          <span class="max-w-52 truncate">{chip.label || toolLabel(name)}</span>
        </button>
      {:else if chip.kind === 'approval'}
        <div
          class="w-full rounded-lg border border-line bg-surface px-3 py-2"
          role="group"
          aria-label={chip.summary || 'Approval required'}
        >
          <div class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
            {chip.status === 'approved' ? 'Approved' : chip.status === 'denied' ? 'Denied' : 'Needs approval'}
          </div>
          <p class="mt-1 font-sans text-[13px] text-fg">{chip.summary || 'A protected action is waiting.'}</p>
          {#if error && busy === null}
            <p class="mt-1 font-sans text-xs text-[color:var(--theme-danger)]">{error}</p>
          {/if}
          {#if chip.status !== 'approved' && chip.status !== 'denied'}
            <div class="mt-2 flex flex-wrap gap-1.5">
              <Button size="xs" variant="outline" disabled={busy !== null} onclick={() => void decide(chip, 'approve')}>
                <Check size={12} /> {busy === chip.actionId ? 'Approving' : 'Approve'}
              </Button>
              <Button size="xs" variant="ghost" disabled={busy !== null} onclick={() => void decide(chip, 'deny')}>
                <X size={12} /> Deny
              </Button>
            </div>
          {/if}
        </div>
      {:else if chip.kind === 'unlock'}
        <div class="inline-flex max-w-full flex-wrap items-center gap-1.5 rounded-md border border-line bg-raised px-2 py-1">
          <span class="font-mono text-[10px] uppercase tracking-[0.05em] text-muted">Unlocked</span>
          {#each chip.tools ?? [] as name (name)}
            <button
              type="button"
              class={cn(
                'rounded border border-line px-1.5 py-0.5 font-sans text-xs text-fg transition-colors hover:border-line-strong hover:bg-hover',
                focusGold,
              )}
              title={toolDescription(name)}
              onclick={() => open({ ...chip, id: `${chip.id}:${name}`, kind: 'tool', tool: name })}
            >
              {toolLabel(name)}
            </button>
          {/each}
        </div>
      {/if}
      {#if openTool === chip.id || (chip.kind === 'unlock' && openTool?.startsWith(`${chip.id}:`))}
        {@const name = chip.kind === 'tool' ? (chip.tool ?? '') : (openTool?.split(':').pop() ?? '')}
        {@const fields = fieldsFor(chip, name)}
        <div class="w-full rounded-lg border border-line bg-surface px-3 py-2">
          <div class="font-sans text-[13px] font-medium text-fg">{toolLabel(name)}</div>
          <p class="mt-0.5 font-sans text-xs text-muted">{chip.description || toolDescription(name)}</p>
          <div class="mt-2 space-y-1.5">
            {#each fields as field (field.name)}
              <label class="block">
                <span class="font-mono text-[10px] uppercase tracking-[0.05em] text-ink-dim">{field.label}</span>
                {#if field.type === 'text'}
                  <textarea
                    class="mt-0.5 w-full rounded-md border border-line bg-raised px-2 py-1 font-sans text-xs text-fg outline-none focus-visible:outline-2 focus-visible:outline-dashed focus-visible:outline-accent"
                    rows="3"
                    bind:value={draft[field.name]}
                  ></textarea>
                {:else}
                  <input
                    class="mt-0.5 w-full rounded-md border border-line bg-raised px-2 py-1 font-sans text-xs text-fg outline-none focus-visible:outline-2 focus-visible:outline-dashed focus-visible:outline-accent"
                    bind:value={draft[field.name]}
                  />
                {/if}
              </label>
            {/each}
          </div>
          {#if onInvoke}
            <div class="mt-2">
              <Button size="xs" onclick={() => run({ ...chip, tool: name })}>Run</Button>
            </div>
          {/if}
        </div>
      {/if}
    {/each}
  </div>
{/if}
