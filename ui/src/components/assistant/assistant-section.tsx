import { useEffect, useState } from 'react'
import { Link } from '@tanstack/react-router'
import { useQueryClient } from '@tanstack/react-query'
import { Sparkles } from 'lucide-react'
import { Avatar } from '@/components/ui/avatar'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { EmptyState } from '@/components/ui/empty-state'
import { AssistantWizard } from './assistant-wizard'
import { AssistantManageModal } from './assistant-manage-modal'
import { InternalEditorModal } from '@/components/fleet/internal-editor-modal'
import { updateAssistant, useAssistant } from '@/lib/assistant'

// Settings › Your assistant — the member-facing controls for their personal
// agent: rename it, rewrite its personality (applies live), or create one via
// the onboarding wizard. Admin internals (models, skills, versions) stay in
// the fleet dashboard.
export function AssistantSection() {
  const qc = useQueryClient()
  const { data: assistant, isLoading } = useAssistant()
  const [wizard, setWizard] = useState(false)
  const [manage, setManage] = useState(false)
  const [personaEditor, setPersonaEditor] = useState(false)
  const [name, setName] = useState('')
  const [personality, setPersonality] = useState('')
  const [busy, setBusy] = useState(false)
  const [saved, setSaved] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (assistant) {
      setName(assistant.displayName)
      setPersonality(assistant.personality ?? '')
    }
  }, [assistant])

  if (isLoading) return null

  if (!assistant) {
    return (
      <section className="mercury-panel mt-6 rounded-2xl p-6">
        <EmptyState
          icon={<Sparkles size={24} />}
          title="No assistant yet"
          hint="A personal agent that's just yours — memory, skills, and tools of its own."
          action={
            <Button size="sm" onClick={() => setWizard(true)}>
              Set up your assistant
            </Button>
          }
        />
        {wizard && <AssistantWizard onClose={() => setWizard(false)} />}
      </section>
    )
  }

  const dirty = name.trim() !== assistant.displayName || personality.trim() !== (assistant.personality ?? '')

  const save = async () => {
    if (!dirty || !name.trim()) return
    setBusy(true)
    setError(null)
    try {
      const r = await updateAssistant({
        ...(name.trim() !== assistant.displayName ? { name: name.trim() } : {}),
        ...(personality.trim() !== (assistant.personality ?? '') ? { personality: personality.trim() } : {}),
      })
      if (!r.assistant) return setError(r.error ?? 'could not save')
      await qc.invalidateQueries({ queryKey: ['my-assistant'] })
      setSaved(true)
      setTimeout(() => setSaved(false), 1500)
    } finally {
      setBusy(false)
    }
  }

  return (
    <section className="mercury-panel mt-6 rounded-2xl p-6">
      <div className="mb-4 flex items-center gap-3">
        <Avatar name={assistant.displayName} className="h-10 w-10" />
        <div className="min-w-0 flex-1">
          <div className="truncate text-sm font-medium text-fg">{assistant.displayName}</div>
          <div className="flex items-center gap-1.5 text-xs text-muted">
            <span
              className="h-1.5 w-1.5 rounded-full"
              style={{ background: assistant.running ? 'var(--theme-success)' : 'var(--theme-line)' }}
            />
            @{assistant.slug} · {assistant.running ? 'online' : 'offline'}
            {assistant.currentModel && <span className="truncate"> · {assistant.currentModel}</span>}
          </div>
        </div>
        <Button variant="outline" size="sm" onClick={() => setManage(true)}>
          Manage
        </Button>
        <Link to="/chat" className="text-xs text-accent hover:underline">
          Open chat →
        </Link>
      </div>

      <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Name</label>
      <Input value={name} onChange={(e) => setName(e.target.value)} maxLength={60} className="mb-4" />

      <div className="mb-1 flex items-center">
        <label className="text-[11px] uppercase tracking-wide text-muted">Personality</label>
        <button type="button" className="ml-auto text-xs text-accent hover:underline" onClick={() => setPersonaEditor(true)}>
          Open editor
        </button>
      </div>
      <Textarea
        rows={4}
        value={personality}
        onChange={(e) => setPersonality(e.target.value)}
        placeholder="How it should come across — tone, priorities, pet peeves."
        maxLength={4000}
      />
      {personaEditor && (
        <InternalEditorModal
          open
          onClose={() => setPersonaEditor(false)}
          title={`${assistant.displayName} · Personality`}
          subtitle="How your assistant comes across. Every save is versioned and applies right away — your assistant restarts with it."
          value={assistant.personality ?? ''}
          editable
          saving={busy}
          onSave={async (md) => {
            setBusy(true)
            try {
              const r = await updateAssistant({ personality: md })
              if (!r.assistant) throw new Error(r.error ?? 'could not save')
              setPersonality(md)
              await qc.invalidateQueries({ queryKey: ['my-assistant'] })
            } finally {
              setBusy(false)
            }
          }}
          history={{ kind: 'personality', id: assistant.id }}
        />
      )}
      <div className="mt-3 flex items-center gap-3">
        <Button size="sm" onClick={() => void save()} disabled={busy || !dirty || !name.trim()}>
          {busy ? 'Saving…' : 'Save'}
        </Button>
        <span className="text-xs text-muted">Changes apply right away — your assistant restarts with them.</span>
      </div>
      {saved && <div className="mt-2 text-xs text-[color:var(--theme-success)]">Saved</div>}
      {error && <div className="mt-2 text-xs text-[color:var(--theme-danger)]">{error}</div>}
      {manage && <AssistantManageModal assistant={assistant} onClose={() => setManage(false)} />}
    </section>
  )
}
