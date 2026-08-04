import { useEffect, useState } from 'react'
import { Link } from '@tanstack/react-router'
import { useQueryClient } from '@tanstack/react-query'
import { Sparkles } from 'lucide-react'
import { Avatar } from '@/components/ui/avatar'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { RichEditor } from '@/components/ui/rich-editor'
import { EmptyState } from '@/components/ui/empty-state'
import { Skeleton } from '@/components/ui/skeleton'
import { AssistantWizard } from './assistant-wizard'
import { AssistantPanels } from './assistant-manage-modal'
import { InternalEditorModal } from '@/components/fleet/internal-editor-modal'
import { cn } from '@/lib/cn'
import { focusGold } from '@/components/chat/chat-chrome'
import { HANDLE_RE, updateAssistant, useAssistant } from '@/lib/assistant'

// §8 field label: 10px mono uppercase 0.08em ink-dim.
const fieldLabel = 'mb-1 block font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim'

// Settings › Assistant — the WHOLE of a member's personal agent in one place:
// identity (name, @handle), personality, which model powers it, on/off, and
// the working parts (schedules, skills, memory) inline below. Owner-scoped
// APIs throughout — no admin role anywhere.
export function AssistantSection() {
  const qc = useQueryClient()
  const { data: assistant, isLoading } = useAssistant()
  const [wizard, setWizard] = useState(false)
  const [personaEditor, setPersonaEditor] = useState(false)
  const [handle, setHandle] = useState('')
  const [power, setPower] = useState(false)
  const [name, setName] = useState('')
  const [personality, setPersonality] = useState('')
  const [personaRev, setPersonaRev] = useState(0)
  const [busy, setBusy] = useState(false)
  const [saved, setSaved] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    if (assistant) {
      setName(assistant.displayName)
      setHandle(assistant.slug)
      setPersonality(assistant.personality ?? '')
      setPersonaRev((r) => r + 1) // reseed the editor with the fetched value
    }
  }, [assistant])

  // The card's shape while we find out whether an assistant exists: header
  // (avatar + name/status), the two identity inputs, model chips, the
  // personality editor block, and the tab strip below the divider.
  if (isLoading)
    return (
      <section aria-hidden className="rounded-lg border border-line bg-panel p-6">
        <div className="mb-4 flex items-center gap-3">
          <Skeleton className="h-10 w-10 shrink-0 rounded-full" />
          <div className="min-w-0 flex-1 space-y-2">
            <Skeleton className="h-3 w-32 rounded-full" delay={0.12} />
            <Skeleton className="h-2.5 w-48 rounded-full" delay={0.24} />
          </div>
          <Skeleton className="h-9 w-16 rounded-lg" delay={0.12} />
        </div>
        <div className="mb-4 grid gap-4 sm:grid-cols-2">
          <div className="space-y-2">
            <Skeleton className="h-2.5 w-12 rounded-full" />
            <Skeleton className="h-11 w-full" delay={0.12} />
          </div>
          <div className="space-y-2">
            <Skeleton className="h-2.5 w-14 rounded-full" delay={0.12} />
            <Skeleton className="h-11 w-full" delay={0.24} />
          </div>
        </div>
        <div className="mb-4 flex flex-wrap gap-1.5">
          <Skeleton className="h-6 w-16 rounded-full" />
          <Skeleton className="h-6 w-16 rounded-full" delay={0.12} />
          <Skeleton className="h-6 w-16 rounded-full" delay={0.24} />
        </div>
        <Skeleton className="mb-6 h-20 w-full rounded-xl" delay={0.12} />
        <div className="flex gap-2 border-t border-line pt-5">
          <Skeleton className="h-8 w-24 rounded-lg" />
          <Skeleton className="h-8 w-16 rounded-lg" delay={0.12} />
          <Skeleton className="h-8 w-16 rounded-lg" delay={0.24} />
        </div>
      </section>
    )

  if (!assistant) {
    return (
      <section className="rounded-lg border border-line bg-panel p-6">
        <EmptyState
          icon={<Sparkles size={24} />}
          title="No assistant yet"
          hint="A personal agent that's just yours: memory, skills, and tools of its own."
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

  const handleDirty = handle.trim() !== assistant.slug && HANDLE_RE.test(handle.trim())
  const dirty = name.trim() !== assistant.displayName || handleDirty || personality.trim() !== (assistant.personality ?? '')

  const save = async () => {
    if (!dirty || !name.trim()) return
    setBusy(true)
    setError(null)
    try {
      const r = await updateAssistant({
        ...(name.trim() !== assistant.displayName ? { name: name.trim() } : {}),
        ...(handleDirty ? { handle: handle.trim() } : {}),
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
    <section className="rounded-lg border border-line bg-panel p-6">
      <div className="mb-4 flex items-center gap-3">
        <Avatar name={assistant.displayName} className="h-10 w-10" />
        <div className="min-w-0 flex-1">
          <div className="truncate text-sm font-medium text-fg">{assistant.displayName}</div>
          {/* Identity meta speaks mono (spec §2 metadata voice); 6px status dot
              green=online, hairline-toned=off (spec §8 status dots). */}
          <div className="flex items-center gap-1.5 font-mono text-[11px] text-muted">
            <span className={cn('h-1.5 w-1.5 rounded-full', assistant.running ? 'bg-success' : 'bg-line')} />
            @{assistant.slug} · {assistant.running ? 'online' : 'offline'}
            {assistant.currentModel && <span className="truncate"> · {assistant.currentModel}</span>}
          </div>
        </div>
        <Button
          variant="outline"
          size="sm"
          disabled={busy}
          onClick={() =>
            void (async () => {
              setPower(true)
              try {
                await fetch(`/api/fleet/agents/${assistant.id}/control`, {
                  method: 'POST',
                  credentials: 'same-origin',
                  headers: { 'content-type': 'application/json' },
                  body: JSON.stringify({ action: assistant.running ? 'stop' : 'up' }),
                })
                await qc.invalidateQueries({ queryKey: ['my-assistant'] })
              } finally {
                setPower(false)
              }
            })()
          }
        >
          {power ? '…' : assistant.running ? 'Stop' : 'Start'}
        </Button>
        <Link to="/chat" className="text-xs text-accent hover:underline">
          Open chat →
        </Link>
      </div>

      <div className="mb-4 grid gap-4 sm:grid-cols-2">
        <div>
          <label className={fieldLabel}>Name</label>
          <Input value={name} onChange={(e) => setName(e.target.value)} maxLength={60} />
        </div>
        <div>
          <label className={fieldLabel}>Handle</label>
          <div className="flex items-center gap-2">
            <span className="text-sm text-muted">@</span>
            <Input
              value={handle}
              onChange={(e) => setHandle(e.target.value.toLowerCase())}
              maxLength={30}
              title="Chats, memory, and access move with it; mentions pick up the new handle"
            />
          </div>
          {handle !== '' && !HANDLE_RE.test(handle) && (
            <p className="mt-1 text-xs text-danger">2–30 lowercase letters/numbers, starting with a letter.</p>
          )}
        </div>
      </div>

      {assistant.tiers.length > 0 && (
        <div className="mb-4">
          <label className={fieldLabel}>Model</label>
          {/* Tier chips per §7/§8 chip anatomy: radius 6, mono 10px uppercase;
              the active tier reads gold, inactive hairline+muted → hover. */}
          <div className="flex flex-wrap gap-1.5">
            {assistant.tiers.map((t) => (
              <button
                key={t.name}
                type="button"
                title={t.model}
                disabled={busy}
                onClick={() =>
                  void updateAssistant({ model: t.name }).then(() => qc.invalidateQueries({ queryKey: ['my-assistant'] }))
                }
                className={cn(
                  'rounded-md border px-2.5 py-1 font-mono text-[10px] uppercase tracking-[0.05em] transition-colors',
                  focusGold,
                  t.active ? 'border-accent text-accent' : 'border-line text-muted hover:bg-hover hover:text-fg',
                )}
              >
                {t.name}
              </button>
            ))}
          </div>
        </div>
      )}

      <div className="mb-1 flex items-center">
        <label className={cn(fieldLabel, 'mb-0')}>Personality</label>
        <button type="button" className="ml-auto text-xs text-accent hover:underline" onClick={() => setPersonaEditor(true)}>
          Open editor
        </button>
      </div>
      {/* Inline rich edit (autosave keeps `personality`/dirty fresh); the
          "Open editor" modal adds muse drafting + version history. Reseeds
          when a modal save lands. */}
      <div className="max-h-64 overflow-y-auto">
        <RichEditor
          key={personaRev}
          value={personality}
          onSave={setPersonality}
          autosave
          minHeight="5rem"
          placeholder="How it should come across: tone, priorities, pet peeves."
        />
      </div>
      {personaEditor && (
        <InternalEditorModal
          open
          onClose={() => setPersonaEditor(false)}
          title={`${assistant.displayName} · Personality`}
          subtitle="How your assistant comes across. Every save is versioned and applies right away. Your assistant restarts with it."
          value={assistant.personality ?? ''}
          editable
          saving={busy}
          muse={{ kind: 'personality', context: `${assistant.displayName}, the user's personal AI assistant.` }}
          onSave={async (md) => {
            setBusy(true)
            try {
              const r = await updateAssistant({ personality: md })
              if (!r.assistant) throw new Error(r.error ?? 'could not save')
              setPersonality(md) // the invalidation below reseeds the inline editor
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
          {busy ? 'Saving' : 'Save'}
        </Button>
        <span className="text-xs text-muted">Changes apply right away. Your assistant restarts with them.</span>
      </div>
      {saved && <div className="mt-2 text-xs text-success">Saved</div>}
      {error && <div className="mt-2 text-xs text-danger">{error}</div>}
      <div className="mt-6 border-t border-line pt-5">
        <AssistantPanels assistant={assistant} />
      </div>
    </section>
  )
}
