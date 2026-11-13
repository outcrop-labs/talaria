<script lang="ts">
  import SettingsField from '@/components/settings/SettingsField.svelte'
  import DangerLink from '@/components/ui/DangerLink.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
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
    /** Deletes the record. The double opt-in (typing the name) is the
     *  caller's. */
    onDelete: () => void
  } = $props()

  // The record's fields, held locally: the view IS the editor now, and NOTHING
  // is written until the record's one Save — a rename, a guidance edit and a
  // skeleton edit land as one write, the way the record's schema takes them.
  // Seeded null-safe: the null frame (see the prop) never touches these.
  // svelte-ignore state_referenced_locally -- reason: record editor fields, one Save per record; the view remounts per selected record
  let name = $state(template?.name ?? '')
  // svelte-ignore state_referenced_locally -- reason: record editor fields, one Save per record; the view remounts per selected record
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
       Muse live here instead of behind an Edit button. -->
  <RecordEditor
    kind={template.kind === 'plan' ? 'Plan template' : 'Ticket template'}
    title={template.name}
    fieldsDirty={name.trim() !== template.name || guidance !== template.guidance}
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
    <div class="mb-1.5 space-y-5">
      <div class="flex items-start gap-6">
        <p class="min-w-0 flex-1 font-sans text-[13px] leading-5 text-muted">{blurb} Edited {relativeTime(template.updatedAt)}.</p>
        <DangerLink onClick={onDelete} class="shrink-0">Delete template</DangerLink>
      </div>
      <SettingsField label="Name" for="template-name">
        <Input id="template-name" size="sm" bind:value={name} class="max-w-sm font-sans" />
      </SettingsField>
      <SettingsField
        label="Agent guidance"
        for="template-guidance"
        hint="Instructions for the agent. Never shown on the ticket or plan itself."
      >
        <Textarea
          id="template-guidance"
          autoGrow
          rows={2}
          bind:value={guidance}
          class="max-h-40"
          placeholder="Always fill acceptance criteria. Keep Out of scope honest."
        />
      </SettingsField>
      <div class="font-sans text-[13px] font-medium leading-5 text-fg">Outline</div>
    </div>
  {/snippet}
</RecordEditor>
{/if}
