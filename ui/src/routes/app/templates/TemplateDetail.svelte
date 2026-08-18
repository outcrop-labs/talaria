<script lang="ts">
  import Button from '@/components/ui/Button.svelte'
  import Input from '@/components/ui/Input.svelte'
  import Textarea from '@/components/ui/Textarea.svelte'
  import Chip from '@/components/ui/Chip.svelte'
  import Markdown from '@/components/ui/Markdown.svelte'
  import InfoTip from '@/components/ui/InfoTip.svelte'
  import InternalEditorModal from '@/components/fleet/InternalEditorModal.svelte'
  import { relativeTime } from '@/lib/fleet'
  import { updateTemplate, type Template } from '@/lib/templates'

  let {
    template,
    blurb,
    onChanged,
    onDelete,
    editorOpen,
    setEditorOpen,
  }: {
    template: Template
    blurb: string
    onChanged: () => void
    onDelete: () => void
    editorOpen: boolean
    setEditorOpen: (v: boolean) => void
  } = $props()

  // Per-template local edit state — the caller keys this component on the
  // template id, so a new selection starts fresh.
  let name = $state(template.name)
  let guidance = $state(template.guidance)
  let busy = $state(false)

  const save = async (patch: { name?: string; guidance?: string; body?: string }) => {
    busy = true
    try {
      await updateTemplate(template.id, patch)
      onChanged()
    } finally {
      busy = false
    }
  }
</script>

<!-- No Panel of its own: this renders INSIDE LibraryPane's surface, and a
     panel within a panel is a border inside a border with the padding twice.
     The pane owns the frame; this owns the content. -->
<div>
  <div class="mb-4 flex items-center gap-2">
    <Input size="sm" bind:value={name} onblur={() => name.trim() && name !== template.name && void save({ name: name.trim() })} class="max-w-xs font-sans" />
    <Chip>{template.kind}</Chip>
    <span class="ml-auto font-mono text-[11px] text-muted">{relativeTime(template.updatedAt)}</span>
  </div>

  <div class="mb-4">
    <div class="mb-1 flex items-center gap-1.5">
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Skeleton</span>
      <InfoTip text={blurb} />
      <Button size="sm" variant="outline" class="ml-auto" onclick={() => setEditorOpen(true)}>
        Edit
      </Button>
    </div>
    {#if template.body.trim()}
      <div class="max-h-80 overflow-y-auto rounded-lg border border-line bg-card px-4 py-3 text-sm">
        <Markdown class="tiptap" children={template.body} />
      </div>
    {:else}
      <button type="button" onclick={() => setEditorOpen(true)} class="w-full rounded-lg border border-dashed border-line px-4 py-6 text-center text-xs text-muted transition-colors hover:border-line-strong hover:text-fg">
        Empty. Open the editor — or let Muse draft it from a description.
      </button>
    {/if}
  </div>

  <div>
    <div class="mb-1 flex items-center gap-1.5">
      <span class="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">Agent guidance</span>
      <InfoTip text="Prompt-only: travels with the template into the model's instructions but is never shown on the ticket or plan itself." />
    </div>
    <Textarea autoGrow rows={2} bind:value={guidance} onblur={() => guidance !== template.guidance && void save({ guidance })} class="max-h-40 text-xs" placeholder={'e.g. "Always fill acceptance criteria; keep Out of scope honest."'} />
  </div>

  {#if editorOpen}
    {#snippet footerExtra()}
      <Button variant="ghost" size="sm" onclick={onDelete}>
        Delete template
      </Button>
    {/snippet}
    <InternalEditorModal
      open
      onClose={() => setEditorOpen(false)}
      title={`${template.name} · skeleton`}
      subtitle="The sections every one of these keeps. Saves are versioned; Muse drafts from a description."
      value={template.body}
      editable
      saving={busy}
      onSave={(md) => void save({ body: md })}
      history={{ kind: 'template', id: template.id }}
      muse={{ kind: 'template', context: `A ${template.kind} template named "${template.name}" for a team workspace.` }}
      {footerExtra}
    />
  {/if}
</div>
