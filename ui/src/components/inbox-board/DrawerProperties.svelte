<script lang="ts">
  import type { Snippet } from 'svelte'
  import { ago, cardMeta, glyphOf, statusText, type InboxCard, type InboxPerson } from '@/lib/inbox-board'
  import Owners from './Owners.svelte'
  import StateGlyph from './StateGlyph.svelte'

  let { card, now, people, due }: { card: InboxCard; now: number; people: readonly InboxPerson[] | null; due: string | null } = $props()
</script>

{#snippet item(label: string, value: Snippet)}
  <div class="min-w-0">
    <dt class="text-[12px] leading-4 text-muted">{label}</dt>
    <dd class="mt-0.5 flex min-h-5 min-w-0 items-center gap-1.5 text-fg">{@render value()}</dd>
  </div>
{/snippet}

{#snippet status()}<StateGlyph glyph={glyphOf(card)} /><span class="truncate">{statusText(cardMeta(card, now))}</span>{/snippet}
{#snippet owners()}<span class="truncate"><Owners people={people!} size="md" /></span>{/snippet}
{#snippet raisedBy()}<span class="truncate"><Owners people={[{ name: card.from!, agent: false }]} size="md" /></span>{/snippet}
{#snippet source()}
  {#if card.where}<span class="truncate">{card.where}</span>{/if}
  {#if card.ref}<span class="shrink-0 font-mono text-[12px] text-muted">{card.ref}</span>{/if}
{/snippet}
{#snippet dueDate()}{new Date(due!).toLocaleDateString(undefined, { day: 'numeric', month: 'short' })}{/snippet}
{#snippet started()}{ago(now - Date.parse(card.startedAt!))}{/snippet}
{#snippet updated()}{ago(now - Date.parse(card.at))}{/snippet}

<div class="@container">
  <dl class="grid grid-cols-2 gap-x-6 gap-y-2.5 font-sans text-[13px] leading-5 @2xl:grid-cols-3">
    {@render item('Status', status)}
    {#if people}{@render item('Owner', owners)}{/if}
    {#if card.from}{@render item('Raised by', raisedBy)}{/if}
    {#if card.where || card.ref}{@render item('Source', source)}{/if}
    {#if due}{@render item('Due', dueDate)}{/if}
    {#if card.startedAt && card.startedAt !== card.at}{@render item('Started', started)}{/if}
    {@render item('Updated', updated)}
  </dl>
</div>
