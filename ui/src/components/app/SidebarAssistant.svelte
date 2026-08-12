<script lang="ts">
  import { Bot, Plus } from '@lucide/svelte'
  import Avatar from '@/components/ui/Avatar.svelte'
  import Skeleton from '@/components/ui/Skeleton.svelte'
  import RailTooltip from './RailTooltip.svelte'
  import { cn } from '@/lib/cn'
  import { useAssistant } from '@/lib/assistant'
  import {
    readPanelCollapsed,
    readPanelUnseen,
    subscribePanelCollapsed,
    subscribePanelUnseen,
    writePanelCollapsed,
    writePanelUnseen,
  } from '@/components/inbox/inbox-chat-panel'

  // THE PERSONAL ASSISTANT'S PLACE IN THE RAIL.
  //
  // The assistant conversation used to advertise itself with a 44px strip of
  // its own, parked between the nav and the page on every view. A second
  // vertical bar is a lot of permanent real estate for one chevron, and it sat
  // outside the menu, so the assistant read as chrome rather than as something
  // the person owns. It belongs in the sidebar, next to everything else they
  // reach for — where the Projects/Tasks meters used to be.
  //
  // This is a LAUNCHER, not a second chat: it says who the assistant is,
  // whether it is awake, and whether it has said something since you last
  // looked. Clicking slides the same panel out the same way.
  let { collapsed = false }: { collapsed?: boolean } = $props()

  const query = useAssistant()
  const assistant = $derived(query.data)

  // Panel chrome state lives in the panel's own module (localStorage + events),
  // so the launcher drives it without either component importing the other.
  let panelOpen = $state(!readPanelCollapsed())
  $effect(() => subscribePanelCollapsed(() => (panelOpen = !readPanelCollapsed())))
  let unseen = $state(readPanelUnseen())
  $effect(() => subscribePanelUnseen(() => (unseen = readPanelUnseen())))

  function open() {
    writePanelCollapsed(false)
    writePanelUnseen(false)
  }
  const toggle = () => (panelOpen ? writePanelCollapsed(true) : open())

  const name = $derived(assistant?.displayName ?? 'Assistant')
  // Three distinct facts, and the dash is one of them: a failed read must not
  // render as "offline", which is a claim about the assistant rather than
  // about our ability to ask.
  const status = $derived(
    query.isError && assistant === undefined ? 'unknown' : !assistant ? 'none' : assistant.running ? 'online' : 'offline',
  )
  const dotClass = $derived(
    status === 'online' ? 'bg-success' : status === 'unknown' ? 'bg-[color:var(--theme-danger)]' : 'bg-line-strong',
  )
  const label = $derived(
    status === 'none'
      ? 'Set up your assistant'
      : status === 'unknown'
        ? `${name} — status unavailable`
        : `${name} — ${status}`,
  )
</script>

{#if collapsed}
  <!-- Icon rail: the same launcher at 36px. Without it the assistant would be
       unreachable in this mode, which is exactly what removing the old strip
       would otherwise have caused. -->
  <RailTooltip label={unseen ? `${label} · new output` : label}>
    {#if status === 'none'}
      <a
        href="/settings?tab=assistant"
        aria-label="Set up your assistant"
        class="relative grid h-9 w-9 place-items-center rounded-md text-muted transition-colors duration-[120ms] hover:bg-hover hover:text-fg"
      >
        <Bot size={16} strokeWidth={1.5} />
      </a>
    {:else}
      <button
        type="button"
        data-assistant-launcher
        onclick={toggle}
        aria-label={label}
        aria-expanded={panelOpen}
        class={cn(
          'relative grid h-9 w-9 place-items-center rounded-md text-muted transition-colors duration-[120ms] hover:bg-hover hover:text-fg',
          panelOpen && 'bg-raised text-fg',
        )}
      >
        <Bot size={16} strokeWidth={1.5} />
        <span class={cn('absolute bottom-1 right-1 h-1.5 w-1.5 rounded-full', dotClass)} aria-hidden="true"></span>
        {#if unseen}
          <span class="absolute right-0 top-0 h-1.5 w-1.5 rounded-full bg-accent" aria-label="New assistant output"></span>
        {/if}
      </button>
    {/if}
  </RailTooltip>
{:else}
  <section aria-label="Your assistant" class="mt-3 shrink-0">
    {#if query.isPending}
      <div class="flex h-11 items-center gap-2.5 rounded-lg border border-line-subtle px-2.5">
        <Skeleton class="h-7 w-7 rounded-full" />
        <div class="min-w-0 flex-1 space-y-1.5">
          <Skeleton class="h-2 w-20 rounded-full" />
          <Skeleton class="h-1.5 w-12 rounded-full" delay={0.08} />
        </div>
      </div>
    {:else if status === 'none'}
      <!-- No assistant yet. The launcher would be a button that opens an empty
           conversation, so it points at the thing that fixes that instead. -->
      <a
        href="/settings?tab=assistant"
        class="flex h-11 items-center gap-2.5 rounded-lg border border-dashed border-line px-2.5 text-left transition-colors duration-[120ms] hover:border-line-strong hover:bg-hover"
      >
        <span class="grid h-7 w-7 shrink-0 place-items-center rounded-full border border-line text-muted"><Plus size={14} strokeWidth={1.5} /></span>
        <span class="min-w-0 flex-1">
          <span class="block truncate font-sans text-[13px] text-fg">Set up your assistant</span>
          <span class="block truncate font-mono text-[9px] uppercase tracking-[0.07em] text-ink-dim">Personal</span>
        </span>
      </a>
    {:else}
      <button
        type="button"
        data-assistant-launcher
        onclick={toggle}
        aria-expanded={panelOpen}
        title={label}
        class={cn(
          'group flex h-11 w-full items-center gap-2.5 rounded-lg border px-2.5 text-left transition-colors duration-[120ms]',
          panelOpen ? 'border-line-strong bg-raised' : 'border-line-subtle hover:border-line hover:bg-hover',
        )}
      >
        <span class="relative shrink-0">
          <Avatar name={name} class="h-7 w-7" />
          <span
            class={cn('absolute -bottom-px -right-px h-2 w-2 rounded-full border border-[color:var(--theme-sidebar)]', dotClass)}
            aria-hidden="true"
          ></span>
        </span>
        <span class="min-w-0 flex-1">
          <span class="block truncate font-sans text-[13px] leading-4 text-fg">{name}</span>
          <span class="block truncate font-mono text-[9px] uppercase tracking-[0.07em] text-ink-dim">
            {status === 'unknown' ? 'Status unavailable' : `@${assistant?.slug ?? ''} · ${status}`}
          </span>
        </span>
        {#if unseen}
          <span
            class="h-1.5 w-1.5 shrink-0 rounded-full bg-accent"
            aria-label="New assistant output"
            title="New since you last looked"
          ></span>
        {/if}
      </button>
    {/if}
  </section>
{/if}
