<script lang="ts">
  import type { Snippet } from 'svelte'
  import SettingsField from '@/components/settings/SettingsField.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import { cn } from '@/lib/cn'
  import { useUiVoice } from '@/lib/ui-voice'

  /** One field of an MCP dialog, in whichever voice the dialog was opened in.
   *  The marketplace, the custom form and the credential editors open from the
   *  settings area (sans) and from an agent's dialog in comms (mono), so the
   *  label is spelled here once per voice instead of in every dialog. */
  let {
    label,
    for: htmlFor,
    hint,
    info,
    required,
    header,
    stored = false,
    children,
  }: {
    label: string
    for?: string
    hint?: string | null
    info?: string
    /** A credential field: true marks it required. Pass false, not nothing,
     *  for an optional credential so the mono label keeps its baseline row. */
    required?: boolean
    /** The header this value is sent as, when the label is not its name. */
    header?: string | null
    /** A value is already stored for this field. */
    stored?: boolean
    children: Snippet
  } = $props()

  const sans = useUiVoice() === 'sans'
  const credential = $derived(required !== undefined)
  const sansInfo = $derived([header ? `Sent as the ${header} header.` : null, info].filter(Boolean).join(' ') || undefined)
</script>

{#if sans}
  <SettingsField label={required && !stored ? `${label} *` : label} for={htmlFor} hint={hint ?? undefined} info={sansInfo}>
    {@render children()}
  </SettingsField>
{:else}
  <div>
    <svelte:element
      this={htmlFor ? 'label' : 'div'}
      for={htmlFor}
      class={cn(
        'mb-1 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim', // voice-ok: the mono label, for the agent dialog in comms
        info ? 'flex items-center gap-1.5' : credential ? 'flex items-baseline gap-1.5' : 'block',
      )}
    >
      {label}
      {#if required && !stored}<span class="text-accent">*</span>{/if}
      {#if header}<span class="normal-case text-ink-dim/70">→ {header}</span>{/if}<!-- voice-ok: mono label -->
      {#if stored}<span class="normal-case text-success/80">set</span>{/if}
      {#if info}<InfoTip text={info} />{/if}
    </svelte:element>
    {@render children()}
    {#if hint}<div class="mt-1 font-sans text-[11px] text-muted/90">{hint}</div>{/if}<!-- voice-ok: mono hint -->
  </div>
{/if}
