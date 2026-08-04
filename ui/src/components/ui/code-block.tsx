import { useState } from 'react'
import { cn } from '@/lib/cn'
import { focusGold } from '@/components/chat/chat-chrome'

// Fenced code block with a language label + copy button. Highlighting is applied
// by rehype-highlight (highlight.js); colours come from the Mercury hljs theme in
// styles.css. Reuse this anywhere code is shown.
export function CodeBlock({
  code,
  language,
  className,
  children,
}: {
  code: string
  language?: string
  className?: string
  children?: React.ReactNode
}) {
  const [copied, setCopied] = useState(false)

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(code)
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    } catch {
      /* clipboard unavailable */
    }
  }

  return (
    // Ground inset well (spec §8): --code-bg + hairline, control radius 6.
    <div className={cn('group my-3 overflow-hidden rounded-md border border-line bg-[var(--code-bg)]', className)}>
      {/* Section-header row: 10px mono uppercase ink-dim label + right-aligned ghost action. */}
      <div className="flex items-center justify-between border-b border-line-subtle px-3 py-1.5">
        <span className="font-mono text-[10px] uppercase tracking-[0.08em] text-ink-dim">{language || 'text'}</span>
        <button
          type="button"
          onClick={copy}
          className={cn(
            'font-mono text-[10px] uppercase tracking-[0.05em] text-muted transition-colors hover:text-fg',
            focusGold,
          )}
        >
          {copied ? 'copied ✓' : 'copy'}
        </button>
      </div>
      <pre className="overflow-x-auto px-4 py-3 font-mono text-[0.85rem] leading-relaxed">
        {children ?? <code>{code}</code>}
      </pre>
    </div>
  )
}
