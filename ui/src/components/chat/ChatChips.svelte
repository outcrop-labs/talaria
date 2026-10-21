<script lang="ts">
  import { Check, ExternalLink, Info, Wrench, X } from '@lucide/svelte'
  import PlatformLinkChip from './PlatformLinkChip.svelte'
  import TodoPlan from './TodoPlan.svelte'
  import AskCard from './AskCard.svelte'
  import { cardKindFor, sourceLabel, subagentGoal, subagentTone, summarizeChange } from './tool-cards'
  import Button from '@/components/ui/Button.svelte'
  import Disclosure from '@/components/ui/Disclosure.svelte'
  import StatusDot from '@/components/ui/StatusDot.svelte'
  import { focusGold } from '@/components/chat/chat-chrome'
  import { attachmentUrl } from '@/lib/attachments'
  import { openFileViewer } from '@/lib/file-viewer.svelte'
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
  // tools, protected-action approvals, unlocks, notices about the row itself,
  // screenshots, delegated children, web sources, and links the agent produced
  // without pasting the URL. Clicking a tool (or an unlocked tool name) opens
  // its inputs inline. Approve/deny stay on the approval chip; a notice has
  // nothing to click.
  //
  // Which SHAPE a chip takes is `cardKindFor` (tool-cards.ts), asked once per
  // chip: four of them need a field the kind alone does not promise, and a
  // card drawn without it is an empty frame.
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
      {@const card = cardKindFor(chip)}
      {#if card === 'document'}
        <!-- A document the agent EDITED, not merely one it linked. The link
             alone makes the reader open the doc and diff it in their head, so
             the card says what moved: a line per change, and the two strings
             for a Replace, where the numbers alone say nothing. -->
        <div class="w-full rounded-lg border border-line bg-surface px-3 py-2">
          <div class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Edited</div>
          <div class="mt-1 -ml-0.5">
            <PlatformLinkChip href={chip.href ?? ''} entity={(chip.entity ?? 'document') as ChipEntity} title={chip.title} />
          </div>
          <!-- The list is BOUNDED, both ways. A batch edit can carry a dozen
               changes and each preview up to 160 chars a side, which is a
               card taller than the transcript it sits in: the list scrolls
               past four or five rows, and each preview line clamps to two.
               The whole text is one click away in the document the chip
               links to — the card's job is the glance. -->
          <ul class="mt-1.5 max-h-64 space-y-1.5 overflow-y-auto">
            {#each chip.changes ?? [] as change, i (`${change.label}-${i}`)}
              <li>
                <div class="font-mono text-[11px] text-muted">{summarizeChange(change)}</div>
                {#if change.preview}
                  <div class="mt-0.5 rounded-lg border border-line-subtle bg-raised px-2 py-1 font-mono text-xs">
                    <div class="line-clamp-2 whitespace-pre-wrap break-words text-[color:var(--theme-danger)]">−{change.preview.old}</div>
                    <div class="line-clamp-2 whitespace-pre-wrap break-words text-[color:var(--theme-success)]">+{change.preview.new}</div>
                  </div>
                {/if}
              </li>
            {/each}
          </ul>
        </div>
      {:else if card === 'image'}
        <!-- A screenshot the agent took, inline like an attachment and opening
             the same in-app viewer (never a new tab: a desktop webview has no
             chrome to save from). -->
        {@const url = attachmentUrl(chip.uploadId ?? '')}
        <div class="w-full">
          <button
            type="button"
            class={cn('block rounded-md', focusGold)}
            title={chip.label || 'Open this image'}
            onclick={() => openFileViewer({ url, title: chip.label || 'Screenshot', contentType: 'image/png' })}
          >
            <img src={url} alt={chip.label ?? ''} class="max-h-48 rounded-md border border-line object-cover" />
          </button>
        </div>
      {:else if card === 'subagent'}
        <!-- A child the agent delegated to, in the approval card's chrome: it
             is several lines about work happening elsewhere, which is exactly
             the thing a person waiting wants to read. The goal is a line of
             its own only when the label is not already carrying it — the
             producer writes "<role> · <goal>", so an unconditional second
             line printed the same sentence twice. -->
        {@const goal = subagentGoal(chip)}
        <div
          class="w-full rounded-lg border border-line bg-surface px-3 py-2"
          role="group"
          aria-label={chip.label ? `Delegated: ${chip.label}` : 'Delegated'}
        >
          <div class="flex items-center gap-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">
            <span>Delegated</span>
            <StatusDot
              status={subagentTone(chip.status)}
              pulse={chip.status === 'running'}
              title={chip.status ?? 'unknown'}
            />
          </div>
          <p class="mt-1 font-sans text-[13px] text-fg">{chip.label || 'A delegated agent'}</p>
          {#if goal}<p class="mt-0.5 font-sans text-xs text-muted">{goal}</p>{/if}
          {#if chip.summary}<p class="mt-1 font-sans text-xs text-fg">{chip.summary}</p>{/if}
          {#if chip.tools?.length}
            <Disclosure class="mt-1.5" title={`${chip.tools.length} ${chip.tools.length === 1 ? 'tool' : 'tools'}`}>
              {#snippet icon()}<span>⚙</span>{/snippet}
              <ul class="space-y-0.5 font-mono text-[11px] text-muted">
                {#each chip.tools as name (name)}
                  <li>{name}</li>
                {/each}
              </ul>
            </Disclosure>
          {/if}
        </div>
      {:else if card === 'source'}
        <!-- A web page the search actually read. Off-platform, so this is the
             one chip that leaves the app: a new tab, and the snippet on hover
             the way a notice carries its sentence. -->
        <a
          href={chip.href}
          target="_blank"
          rel="noopener noreferrer"
          title={chip.detail || chip.href}
          class={cn(
            'inline-flex max-w-full items-center gap-1.5 rounded-md border border-line bg-raised px-2 py-1 font-sans text-xs text-muted transition-colors hover:border-line-strong hover:bg-hover hover:text-fg',
            focusGold,
          )}
        >
          <ExternalLink size={13} class="shrink-0 text-ink-dim" />
          <span class="max-w-52 truncate">{sourceLabel(chip)}</span>
        </a>
      {:else if chip.kind === 'link' && chip.href && chip.entity}
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
      {:else if chip.kind === 'todo'}
        <!-- The agent's own plan, as a timeline. Full width like an approval
             rather than an inline pill: it is several lines and it is the
             thing a person waiting actually wants to read. -->
        <TodoPlan items={chip.items ?? []} label={chip.label} />
      {:else if chip.kind === 'question'}
        <!-- The agent asked something and is waiting. Answering sends the
             option's own words through the same send path the tool chips use,
             so the reply lands as an ordinary message. -->
        <AskCard
          question={chip.label ?? ''}
          detail={chip.description}
          options={chip.options ?? []}
          onAnswer={onInvoke}
        />
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
      {:else}
        <!-- A fact about the row, in the muted voice: what happened to this
             message, not something to do about it. So it is a span and not a
             button — a notice with an action would be an approval. The detail
             is the title, because the pill has to stay short enough to read
             beside the words it belongs to.

             AND IT IS THE LAST RESORT, not only the `notice` branch. Every
             chip above needs a field its kind does not promise — an upload,
             an href, an entity — and a chip that reaches the end of the chain
             with none of them used to render NOTHING AT ALL: the row the
             server persisted simply was not on screen, which is a worse lie
             than a muted pill saying the little it knows. -->
        <span
          class="inline-flex max-w-full items-center gap-1.5 rounded-md border border-line bg-raised px-2 py-1 font-sans text-xs text-muted"
          title={chip.detail || undefined}
        >
          <Info size={13} class="shrink-0 text-ink-dim" />
          <span class="truncate">{chip.label || chip.title}</span>
        </span>
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
