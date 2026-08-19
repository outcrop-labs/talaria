<script lang="ts">
  import Input from '@/components/ui/Input.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import RecordEditor from '@/components/editor/RecordEditor.svelte'
  import { relativeTime } from '@/lib/fleet'
  import { draftTemplateForm } from '@/lib/muse.svelte'
  import { updateTemplate, type Template } from '@/lib/templates'

  let {
    template,
    blurb,
    onChanged,
    onDelete,
  }: {
    /** Nullable: a refresh can drop the selected template (deleted elsewhere,
     *  filtered out) in the same tick the keyed detail is still mounted — a
     *  null render is one blank frame, not a crash. */
    template: Template | null
    blurb: string
    onChanged: () => void
    /** Deletes the record — rendered as the trash icon on the record's title
     *  line by `<RecordEditor>`; the double opt-in (typing the name) is here. */
    onDelete: () => void
  } = $props()

  // The record's fields, held locally: the view IS the editor now, and NOTHING
  // is written until the record's one Save — a rename, a guidance edit and a
  // skeleton edit land as one write, the way the record's schema takes them.
  // Seeded null-safe: the null frame (see the prop) never touches these.
  let name = $state(template?.name ?? '')
  let guidance = $state(template?.guidance ?? '')
  let busy = $state(false)

  // `updateTemplate` throws on a refused write; the error escapes to
  // `<RecordEditor>`'s save, which shows it next to the (still-dirty) fields.
  const saveAll = async (body: string) => {
    if (!template) return
    busy = true
    try {
      await updateTemplate(template.id, { name: name.trim() || template.name, guidance, body })
      await onChanged()
    } finally {
      busy = false
    }
  }

  const museContext = $derived(
    template
      ? `A ${template.kind} template named "${name}" for a team workspace. The template seeds work of its kind: its name is short and human, its guidance is prompt-only, and its body is the skeleton.`
      : '',
  )
</script>

{#if template}
  <!-- No Panel of its own: this renders INSIDE LibraryPane's surface, and a
       panel within a panel is a border inside a border with the padding twice.
       The pane owns the frame; this owns the content — and editing is the
       content: the record's fields, its document workbench and its whole-form
       Muse live here instead of behind an Edit button. Deletion lives in the
       page's sidebar (trash icon beside the history toggle), so nothing
       destructive rides on this surface. -->
  <RecordEditor
    kind="template"
    title={template.name}
    meta={relativeTime(template.updatedAt)}
    subtitle={blurb}
    fieldsDirty={name.trim() !== template.name || guidance !== template.guidance}
    onDelete={() => onDelete()}
  doc={{
    value: template.body,
    saving: busy,
    onSave: (md: string) => saveAll(md),
    history: { kind: 'template', id: template.id },
    // No doc.muse: the whole-form Muse drafts the complete record — name,
    // guidance AND skeleton — so a second composer in the workbench would be
    // two answers to the same question.
  }}
  formMuse={{
    label: 'template',
    current: (docText) => ({ name, guidance, body: docText }),
    draft: async (input, signal) => draftTemplateForm({ ...input, context: museContext }, signal),
    fields: (d: { name?: string; guidance?: string; body?: string; error?: string }) => [
      { label: 'name', value: String(d.name ?? '').slice(0, 40) },
      { label: 'guidance', value: String(d.guidance ?? '').slice(0, 60) },
    ],
    docOf: (d: { body?: string }) => String(d.body ?? ''),
    apply: (d: { name?: string; guidance?: string }) => {
      const n = String(d.name ?? '').trim()
      if (n) name = n
      if (typeof d.guidance === 'string') guidance = d.guidance
    },
  }}
  onCancel={() => {
    name = template.name
    guidance = template.guidance
  }}
  class="h-full"
>
  {#snippet fields(_)}
    <div class="mb-4 shrink-0 space-y-3 rounded-lg border border-line bg-card/40 p-3">
      <div>
        <div class="mb-1.5 font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Name</div>
        <Input size="sm" bind:value={name} class="max-w-sm font-sans" />
      </div>
      <div>
        <div class="mb-1.5 flex items-center gap-1.5">
          <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Agent guidance</span>
          <InfoTip text="Prompt-only: travels with the template into the model's instructions but is never shown on the ticket or plan itself." />
        </div>
        <Textarea autoGrow rows={2} bind:value={guidance} class="max-h-40 text-xs" placeholder={'e.g. "Always fill acceptance criteria; keep Out of scope honest."'} />
      </div>
    </div>
  {/snippet}
</RecordEditor>
{/if}
