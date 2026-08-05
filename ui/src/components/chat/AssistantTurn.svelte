<script lang="ts">
  import MessageAvatar from './MessageAvatar.svelte'
  import ToolStatus from './ToolStatus.svelte'
  import GuardCaveat from '@/components/chat/GuardCaveat.svelte'
  import Markdown from '@/components/ui/Markdown.svelte'
  import Disclosure from '@/components/ui/Disclosure.svelte'
  import GeneratingDots from '@/components/ui/GeneratingDots.svelte'
  import GeneratingHelix from '@/components/ui/GeneratingHelix.svelte'
  import { resolveAgentMedia } from '@/lib/agent-media'
  import type { DisplayMessage } from './chat-view'

  // Flattened assistant turn (spec §10): avatar square + agent name, compact
  // mono tool rows, 14px sans body.
  let {
    message,
    agentModel,
    agentLabel,
    live,
    onContextMenu,
  }: {
    message: DisplayMessage
    agentModel: string
    agentLabel: string
    live: boolean
    onContextMenu?: (e: MouseEvent) => void
  } = $props()

  const hasReasoning = $derived(!!message.reasoning?.trim())
  const tools = $derived(message.tools ?? [])
  const hasTools = $derived(tools.length > 0)
  const empty = $derived(!message.content && !hasReasoning && !hasTools)
</script>

<div class="flex gap-2.5" oncontextmenu={onContextMenu}>
  <MessageAvatar name={agentLabel} class="mt-0.5" />
  <div class="min-w-0 flex-1 space-y-2">
    <div class="flex items-baseline gap-2">
      <span class="font-sans text-[13px] font-medium text-fg">{agentLabel}</span>
      <span class="rounded border border-line px-1 font-mono text-[9px] uppercase tracking-[0.08em] text-muted">
        agent
      </span>
    </div>

    {#if hasReasoning}
      <Disclosure title="Thinking">
        {#snippet icon()}<span>✦</span>{/snippet}
        <div class="whitespace-pre-wrap text-xs text-muted">{message.reasoning}</div>
      </Disclosure>
    {/if}

    {#if hasTools}
      <Disclosure title={`${tools.length} tool ${tools.length === 1 ? 'call' : 'calls'}`}>
        {#snippet icon()}<span>⚙</span>{/snippet}
        <!-- Compact mono tool rows, hairline separated (spec §10). -->
        <ul class="divide-y divide-line">
          {#each tools as t, i (t.id ?? `${t.name}-${i}`)}
            <li class="flex items-start gap-2 py-1.5 font-mono text-[11px] first:pt-0 last:pb-0">
              <span class="mt-0.5 shrink-0">
                <ToolStatus status={t.status} />
              </span>
              <span class="min-w-0 flex-1">
                <span class="text-fg">{t.name}</span>
                {#if t.label}
                  <span class="mt-0.5 block whitespace-pre-wrap break-words text-muted">{t.label}</span>
                {/if}
              </span>
            </li>
          {/each}
        </ul>
      </Disclosure>
    {/if}

    {#if message.content}
      <div class="font-sans text-sm text-fg">
        <Markdown children={resolveAgentMedia(message.content, agentModel)} />
      </div>
    {/if}
    {#if !live}<GuardCaveat findings={message.guard} />{/if}

    <!-- Spec §9 state mapping: submitting (awaiting the first token) rides
        the fast gold SIGNAL WEAVE; once reasoning/tools stream but prose
        hasn't landed, the CONTEXT HELIX loops on the standard budget. -->
    {#if empty && live}<GeneratingDots class="py-1" />{/if}
    {#if !empty && !message.content && live}<GeneratingHelix class="py-1" />{/if}
    {#if message.content && live}<span class="gd-pulse ml-0.5 inline-block h-4 w-1.5 bg-accent align-middle"></span>{/if}
    {#if !live && message.status === 'streaming'}
      <div class="font-mono text-[11px] text-muted">· saved (was in progress)</div>
    {/if}
    {#if !live && message.status === 'error'}
      <div class="text-xs" style:color="var(--theme-danger)">
        {empty
          ? 'The agent returned nothing. Its model may not be routable; check its config and /models.'
          : '· interrupted'}
      </div>
    {/if}
  </div>
</div>
