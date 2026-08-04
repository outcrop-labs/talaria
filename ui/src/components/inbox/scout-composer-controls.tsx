import { useEffect, useMemo, useRef, useState, type Dispatch, type SetStateAction } from 'react'
import { createPortal } from 'react-dom'
import { CircleHelp, Mic, MicOff, PlugZap, Sparkles } from 'lucide-react'
import { AgentChip } from '@/components/chat/agent-picker'
import { AttachButton } from '@/components/chat/attachments'
import {
  MeterBars,
  PopSearch,
  chipPrimary,
  chipSecondary,
  popHeader,
  popPanel,
  popRow,
  popRowSelected,
  tileBase,
} from '@/components/chat/chat-chrome'
import { cn } from '@/lib/cn'
import type { Attachment } from '@/lib/attachments'
import type { GatewayModel } from '@/lib/muse'

export type ScoutMode = 'normal' | 'fast' | 'plan'

interface RailItem {
  id: string
  label: string
  detail?: string
}

function useAnchoredPopover(open: boolean, setOpen: Dispatch<SetStateAction<boolean>>) {
  const buttonRef = useRef<HTMLButtonElement>(null)
  const panelRef = useRef<HTMLDivElement>(null)
  const [position, setPosition] = useState<{ left: number; bottom: number } | null>(null)

  useEffect(() => {
    if (!open) return
    const place = () => {
      const rect = buttonRef.current?.getBoundingClientRect()
      if (rect) setPosition({ left: rect.left, bottom: window.innerHeight - rect.top + 6 })
    }
    const dismiss = (event: MouseEvent) => {
      const target = event.target as Node
      if (!buttonRef.current?.contains(target) && !panelRef.current?.contains(target)) {
        setOpen(false)
      }
    }
    place()
    window.addEventListener('resize', place)
    window.addEventListener('scroll', place, true)
    document.addEventListener('mousedown', dismiss)
    return () => {
      window.removeEventListener('resize', place)
      window.removeEventListener('scroll', place, true)
      document.removeEventListener('mousedown', dismiss)
    }
  }, [open, setOpen])

  return { buttonRef, panelRef, position }
}

function modelLabel(model: GatewayModel | undefined, fallback: string): string {
  const label = model?.label?.trim() || model?.id || fallback
  return label.replace(/^claude[- ]?/i, '').replace(/^anthropic[/: -]*/i, '')
}

function ModelPicker({
  models,
  value,
  onChange,
  loading,
}: {
  models: GatewayModel[]
  value: string
  onChange: (model: string) => void
  loading: boolean
}) {
  const [open, setOpen] = useState(false)
  const [query, setQuery] = useState('')
  const { buttonRef, panelRef, position } = useAnchoredPopover(open, setOpen)
  const current = models.find((model) => model.id === value)
  const visible = models.filter((model) => {
    const needle = query.trim().toLowerCase()
    return !needle || model.id.toLowerCase().includes(needle) || model.label?.toLowerCase().includes(needle)
  })
  const selectedIndex = Math.max(0, models.findIndex((model) => model.id === value))

  return (
    <>
      <button
        ref={buttonRef}
        type="button"
        disabled={loading || models.length === 0}
        onClick={() => {
          setQuery('')
          setOpen((currentOpen) => !currentOpen)
        }}
        className={cn(chipPrimary, 'w-[108px] justify-between disabled:opacity-40')}
        title="Model for Scout's response"
      >
        <span aria-hidden className="text-accent">✦</span>
        <span className="min-w-0 flex-1 truncate text-left">{loading ? 'Loading' : modelLabel(current, 'Model')}</span>
        <MeterBars total={3} lit={Math.max(1, Math.ceil(((selectedIndex + 1) / Math.max(1, models.length)) * 3))} />
      </button>
      {open && position && typeof document !== 'undefined' && createPortal(
        <div ref={panelRef} className={cn(popPanel, 'fixed z-[70] w-72 overflow-hidden')} style={position}>
          <PopSearch value={query} onChange={setQuery} placeholder="Search models" />
          <div className={popHeader}>Available models</div>
          <div className="max-h-72 overflow-y-auto">
            {visible.map((model) => (
              <button
                key={model.id}
                type="button"
                onClick={() => {
                  onChange(model.id)
                  setOpen(false)
                }}
                className={cn(popRow, model.id === value ? popRowSelected : 'text-muted')}
              >
                <span aria-hidden className="text-accent">✦</span>
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-fg">{model.label || model.id}</span>
                  {model.label && <span className="block truncate font-mono text-[9px] uppercase tracking-[0.05em] text-ink-dim">{model.id}</span>}
                </span>
              </button>
            ))}
            {visible.length === 0 && <div className="px-2 py-2 font-sans text-[13px] text-muted">No models found</div>}
          </div>
        </div>,
        document.body,
      )}
    </>
  )
}

const MODE_OPTIONS: Array<{ id: ScoutMode; label: string; detail: string }> = [
  { id: 'normal', label: 'Normal mode', detail: 'Balanced response with safe action proposals.' },
  { id: 'fast', label: 'Fast mode', detail: 'Prefer the quickest deterministic safe response.' },
  { id: 'plan', label: 'Plan mode', detail: 'Plan and clarify without proposing execution.' },
]

function ModePicker({ value, onChange }: { value: ScoutMode; onChange: (mode: ScoutMode) => void }) {
  const [open, setOpen] = useState(false)
  const { buttonRef, panelRef, position } = useAnchoredPopover(open, setOpen)
  return (
    <>
      <button
        ref={buttonRef}
        type="button"
        onClick={() => setOpen((current) => !current)}
        className={cn(chipSecondary, 'w-24 justify-center')}
        title="Scout response mode"
      >
        {value} mode
      </button>
      {open && position && typeof document !== 'undefined' && createPortal(
        <div ref={panelRef} className={cn(popPanel, 'fixed z-[70] w-64')} style={position}>
          <div className={popHeader}>Response mode</div>
          {MODE_OPTIONS.map((option) => (
            <button
              key={option.id}
              type="button"
              onClick={() => {
                onChange(option.id)
                setOpen(false)
              }}
              className={cn(popRow, option.id === value ? popRowSelected : 'text-muted')}
            >
              <span className="min-w-0 flex-1">
                <span className="block text-fg">{option.label}</span>
                <span className="block text-[11px] leading-4 text-ink-dim">{option.detail}</span>
              </span>
            </button>
          ))}
        </div>,
        document.body,
      )}
    </>
  )
}

function CapabilityPicker({ kind, items }: { kind: 'mcp' | 'skills'; items: RailItem[] }) {
  const [open, setOpen] = useState(false)
  const [query, setQuery] = useState('')
  const { buttonRef, panelRef, position } = useAnchoredPopover(open, setOpen)
  const visible = useMemo(() => {
    const needle = query.trim().toLowerCase()
    return items.filter((item) => !needle || item.label.toLowerCase().includes(needle) || item.detail?.toLowerCase().includes(needle))
  }, [items, query])
  const width = kind === 'mcp' ? 'w-[76px]' : 'w-[70px]'
  return (
    <>
      <button
        ref={buttonRef}
        type="button"
        onClick={() => {
          setQuery('')
          setOpen((current) => !current)
        }}
        className={cn(chipSecondary, width, 'justify-center gap-1.5')}
        title={kind === 'mcp' ? 'MCP access for the selected agent' : 'Skills available to the selected agent'}
      >
        {kind === 'mcp' && <span className="h-1.5 w-1.5 rounded-full bg-accent" aria-hidden />}
        {kind === 'skills' && <Sparkles size={11} aria-hidden />}
        <span>{kind} {items.length}</span>
      </button>
      {open && position && typeof document !== 'undefined' && createPortal(
        <div ref={panelRef} className={cn(popPanel, 'fixed z-[70] w-64 overflow-hidden')} style={position}>
          <PopSearch value={query} onChange={setQuery} placeholder={kind === 'mcp' ? 'Search MCPs' : 'Search skills'} />
          <div className={popHeader}>{kind === 'mcp' ? 'Agent MCP access' : 'Available skills'}</div>
          <div className="max-h-64 overflow-y-auto">
            {visible.map((item) => (
              <div key={item.id} className={cn(popRow, 'text-muted')}>
                {kind === 'mcp' ? <PlugZap size={12} className="shrink-0 text-accent" /> : <Sparkles size={12} className="shrink-0 text-accent" />}
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-fg">{item.label}</span>
                  {item.detail && <span className="block truncate font-mono text-[9px] uppercase tracking-[0.05em] text-ink-dim">{item.detail}</span>}
                </span>
              </div>
            ))}
            {visible.length === 0 && <div className="px-2 py-2 font-sans text-[13px] text-muted">None available</div>}
          </div>
        </div>,
        document.body,
      )}
    </>
  )
}

function HelpButton() {
  const [open, setOpen] = useState(false)
  const { buttonRef, panelRef, position } = useAnchoredPopover(open, setOpen)
  return (
    <>
      <button ref={buttonRef} type="button" onClick={() => setOpen((current) => !current)} className={tileBase} title="Composer help">
        <CircleHelp size={14} />
      </button>
      {open && position && typeof document !== 'undefined' && createPortal(
        <div ref={panelRef} className={cn(popPanel, 'fixed z-[70] w-64 p-3')} style={position}>
          <div className="font-sans text-[13px] font-medium text-fg">Scout composer</div>
          <div className="mt-2 space-y-1.5 font-sans text-[11px] leading-4 text-muted">
            <p>Enter sends. Shift + Enter adds a line.</p>
            <p>Attached decisions keep Talaria's action allowlist and confirmation rules.</p>
            <p>Plan mode never proposes execution.</p>
          </div>
        </div>,
        document.body,
      )}
    </>
  )
}

interface SpeechRecognitionLike {
  continuous: boolean
  interimResults: boolean
  lang: string
  onresult: ((event: { results: ArrayLike<{ 0: { transcript: string }; isFinal: boolean }> }) => void) | null
  onend: (() => void) | null
  onerror: (() => void) | null
  start: () => void
  stop: () => void
}

type SpeechRecognitionConstructor = new () => SpeechRecognitionLike

function VoiceButton({ onTranscript, disabled }: { onTranscript: (text: string) => void; disabled: boolean }) {
  const [recording, setRecording] = useState(false)
  const [supported, setSupported] = useState(true)
  const recognitionRef = useRef<SpeechRecognitionLike | null>(null)

  useEffect(() => () => recognitionRef.current?.stop(), [])

  const toggle = () => {
    if (recording) {
      recognitionRef.current?.stop()
      setRecording(false)
      return
    }
    const speechWindow = window as unknown as {
      SpeechRecognition?: SpeechRecognitionConstructor
      webkitSpeechRecognition?: SpeechRecognitionConstructor
    }
    const Constructor = speechWindow.SpeechRecognition ?? speechWindow.webkitSpeechRecognition
    if (!Constructor) {
      setSupported(false)
      return
    }
    const recognition = new Constructor()
    recognition.continuous = true
    recognition.interimResults = false
    recognition.lang = navigator.language || 'en-US'
    recognition.onresult = (event) => {
      const transcript = Array.from(event.results)
        .filter((result) => result.isFinal)
        .map((result) => result[0]?.transcript ?? '')
        .join(' ')
        .trim()
      if (transcript) onTranscript(`${transcript} `)
    }
    recognition.onend = () => setRecording(false)
    recognition.onerror = () => setRecording(false)
    recognitionRef.current = recognition
    recognition.start()
    setRecording(true)
    setSupported(true)
  }

  return (
    <button
      type="button"
      disabled={disabled}
      onClick={toggle}
      className={cn(tileBase, recording && 'border-accent bg-accent-subtle text-accent')}
      title={supported ? (recording ? 'Stop dictation' : 'Start voice dictation') : 'Voice dictation is not available in this browser'}
      aria-pressed={recording}
    >
      {supported ? <Mic size={14} /> : <MicOff size={14} />}
    </button>
  )
}

export function ScoutComposerControls({
  agents,
  agentValue,
  onAgentChange,
  models,
  modelValue,
  onModelChange,
  modelsLoading,
  mode,
  onModeChange,
  mcpItems,
  skillItems,
  onAttach,
  onTranscript,
  disabled,
}: {
  agents: Array<{ id: string; label: string; role?: string }>
  agentValue: string
  onAgentChange: (value: string) => void
  models: GatewayModel[]
  modelValue: string
  onModelChange: (value: string) => void
  modelsLoading: boolean
  mode: ScoutMode
  onModeChange: (mode: ScoutMode) => void
  mcpItems: RailItem[]
  skillItems: RailItem[]
  onAttach: (attachment: Attachment) => void
  onTranscript: (text: string) => void
  disabled: boolean
}) {
  return (
    <div className="flex min-w-[608px] items-center gap-1.5">
      <AttachButton onAttach={onAttach} disabled={disabled} />
      <AgentChip agents={agents} value={agentValue} onChange={onAgentChange} className="w-[108px] justify-between" />
      <ModelPicker models={models} value={modelValue} onChange={onModelChange} loading={modelsLoading} />
      <ModePicker value={mode} onChange={onModeChange} />
      <CapabilityPicker kind="mcp" items={mcpItems} />
      <CapabilityPicker kind="skills" items={skillItems} />
      <span className="flex-1" />
      <HelpButton />
      <VoiceButton onTranscript={onTranscript} disabled={disabled} />
    </div>
  )
}
