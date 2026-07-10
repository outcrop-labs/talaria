import { useState } from 'react'
import { useNavigate } from '@tanstack/react-router'
import { useQueryClient } from '@tanstack/react-query'
import { Loader2, Sparkles } from 'lucide-react'
import { Modal } from '@/components/ui/modal'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Textarea } from '@/components/ui/textarea'
import { Steps } from '@/components/ui/steps'
import { cn } from '@/lib/cn'
import { useSession } from '@/lib/session'
import { HANDLE_RE, createAssistant, suggestHandle, type Assistant } from '@/lib/assistant'

// Onboarding for the personal assistant: Name (+handle) → Personality → Launch.
// Everything is optional-with-good-defaults so the flow never blocks on ideas —
// but every choice that matters (name, handle, voice) is theirs to make.
const STEPS = ['Name', 'Personality', 'Launch'] as const

const PRESETS = [
  {
    label: 'Warm & proactive',
    text: 'Be warm and personable. Anticipate what I need, suggest next steps, and check in when something is ambiguous rather than guessing.',
  },
  {
    label: 'Concise & professional',
    text: 'Be brief and businesslike. Lead with the answer, skip pleasantries, and use short bullet points for any detail.',
  },
  {
    label: 'Playful & curious',
    text: 'Keep it light — a little wit is welcome. Be curious, offer ideas I did not ask for when they are good, but stay useful first.',
  },
] as const

export function AssistantWizard({ onClose }: { onClose: () => void }) {
  const qc = useQueryClient()
  const navigate = useNavigate()
  const { data: session } = useSession()
  const first = session?.name?.split(' ')[0] ?? session?.email?.split('@')[0] ?? 'My'

  const [step, setStep] = useState(0)
  const [name, setName] = useState(`${first}'s assistant`)
  const [handle, setHandle] = useState(suggestHandle(`${first}'s assistant`))
  const [handleTouched, setHandleTouched] = useState(false)
  const [personality, setPersonality] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [created, setCreated] = useState<Assistant | null>(null)

  const handleOk = HANDLE_RE.test(handle)
  const nameOk = name.trim().length > 0

  const rename = (v: string) => {
    setName(v)
    if (!handleTouched) setHandle(suggestHandle(v))
  }

  const launch = async () => {
    setBusy(true)
    setError(null)
    try {
      const r = await createAssistant({ name: name.trim(), handle, personality: personality.trim() || undefined })
      if (!r.assistant) return setError(r.error ?? 'could not create your assistant')
      setCreated(r.assistant)
      await qc.invalidateQueries({ queryKey: ['my-assistant'] })
      await qc.invalidateQueries({ queryKey: ['agents'] })
    } finally {
      setBusy(false)
    }
  }

  const footer = created ? (
    <div className="flex justify-end gap-2">
      <Button variant="ghost" size="sm" onClick={onClose}>
        Done
      </Button>
      <Button
        size="sm"
        onClick={() => {
          onClose()
          void navigate({ to: '/chat' })
        }}
      >
        Say hello
      </Button>
    </div>
  ) : step === 0 ? (
    <div className="flex justify-end gap-2">
      <Button variant="ghost" size="sm" onClick={onClose}>
        Cancel
      </Button>
      <Button size="sm" onClick={() => setStep(1)} disabled={!nameOk || !handleOk}>
        Continue
      </Button>
    </div>
  ) : step === 1 ? (
    <div className="flex justify-between gap-2">
      <Button variant="ghost" size="sm" onClick={() => setStep(0)}>
        Back
      </Button>
      <Button size="sm" onClick={() => setStep(2)}>
        Continue
      </Button>
    </div>
  ) : (
    <div className="flex justify-between gap-2">
      <Button variant="ghost" size="sm" onClick={() => setStep(1)} disabled={busy}>
        Back
      </Button>
      <Button size="sm" onClick={() => void launch()} disabled={busy}>
        {busy && <Loader2 size={14} className="animate-spin" />}
        {busy ? 'Creating' : 'Create assistant'}
      </Button>
    </div>
  )

  return (
    <Modal open onClose={onClose} title="Set up your assistant" width="max-w-lg" footer={footer}>
      <div className="space-y-5">
        {!created && <Steps steps={STEPS} current={step} />}

        {created ? (
          <div className="space-y-2 py-2 text-center">
            <span className="mx-auto grid h-11 w-11 place-items-center rounded-2xl bg-accent/15 text-accent">
              <Sparkles size={20} />
            </span>
            <div className="text-sm font-medium text-fg">{created.displayName} is ready</div>
            <p className="text-xs text-muted">
              Its workspace is starting up — it has its own memory, skills, and private knowledge, and it only works
              for you. Tune it any time in Settings.
            </p>
          </div>
        ) : step === 0 ? (
          <div className="space-y-4">
            <p className="text-sm text-muted">
              Your assistant is a real agent that's just yours — its own memory, skills, and tools. Start by naming it.
            </p>
            <div>
              <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Name</label>
              <Input value={name} onChange={(e) => rename(e.target.value)} placeholder="Maxie" autoFocus maxLength={60} />
            </div>
            <div>
              <label className="mb-1 block text-[11px] uppercase tracking-wide text-muted">Handle</label>
              <div className="flex items-center gap-2">
                <span className="text-sm text-muted">@</span>
                <Input
                  value={handle}
                  onChange={(e) => {
                    setHandleTouched(true)
                    setHandle(e.target.value.toLowerCase())
                  }}
                  placeholder="maxie"
                  maxLength={30}
                />
              </div>
              <p className={cn('mt-1 text-xs', handle && !handleOk ? 'text-[color:var(--theme-danger)]' : 'text-muted')}>
                {handle && !handleOk
                  ? 'Lowercase letters and numbers only, starting with a letter (2–30 characters).'
                  : "How agents and integrations refer to it — can't be changed later."}
              </p>
            </div>
          </div>
        ) : step === 1 ? (
          <div className="space-y-3">
            <p className="text-sm text-muted">
              How should {name.trim() || 'it'} come across? Pick a starting point or write your own — you can refine it
              any time.
            </p>
            <div className="flex flex-wrap gap-2">
              {PRESETS.map((p) => (
                <button
                  key={p.label}
                  type="button"
                  onClick={() => setPersonality(p.text)}
                  className={cn(
                    'rounded-full border px-3 py-1 text-xs transition-colors',
                    personality === p.text
                      ? 'border-[var(--theme-accent)] text-accent'
                      : 'border-line-subtle text-muted hover:border-line hover:text-fg',
                  )}
                >
                  {p.label}
                </button>
              ))}
            </div>
            <Textarea
              autoGrow
              rows={3}
              className="max-h-60"
              value={personality}
              onChange={(e) => setPersonality(e.target.value)}
              placeholder="Optional — e.g. “Be direct and keep things short. Remind me about loose ends.”"
              maxLength={4000}
            />
          </div>
        ) : (
          <div className="space-y-3">
            <p className="text-sm text-muted">Ready to go. Creating it starts a private workspace — this can take a minute.</p>
            <dl className="space-y-2 rounded-lg border border-line-subtle p-3 text-sm">
              <div className="flex gap-2">
                <dt className="w-24 shrink-0 text-xs uppercase tracking-wide text-muted">Name</dt>
                <dd className="text-fg">{name.trim()}</dd>
              </div>
              <div className="flex gap-2">
                <dt className="w-24 shrink-0 text-xs uppercase tracking-wide text-muted">Handle</dt>
                <dd className="text-fg">@{handle}</dd>
              </div>
              <div className="flex gap-2">
                <dt className="w-24 shrink-0 text-xs uppercase tracking-wide text-muted">Personality</dt>
                <dd className="min-w-0 text-fg">{personality.trim() || <span className="text-muted">Warm, direct, and useful (default)</span>}</dd>
              </div>
            </dl>
            {error && <p className="text-xs text-[color:var(--theme-danger)]">{error}</p>}
          </div>
        )}
      </div>
    </Modal>
  )
}
