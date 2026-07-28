import { Info } from 'lucide-react'
import { cn } from '@/lib/cn'

/** An ⓘ that explains an area on hover — the home for the sentence that used
 *  to sit under a section header cluttering it. Keep the text to one or two
 *  sentences; anything longer belongs in docs. */
export function InfoTip({ text, className }: { text: string; className?: string }) {
  return (
    <span className={cn('group relative inline-flex align-middle', className)}>
      <Info size={13} className="cursor-help text-muted transition-colors group-hover:text-fg" />
      <span
        role="tooltip"
        className="pointer-events-none absolute left-1/2 top-full z-[70] mt-1.5 w-64 -translate-x-1/2 rounded-lg border border-line bg-card px-2.5 py-2 font-sans text-[11px] font-normal normal-case leading-snug tracking-normal text-muted opacity-0 shadow-lg transition-opacity duration-100 group-hover:opacity-100"
      >
        {text}
      </span>
    </span>
  )
}
