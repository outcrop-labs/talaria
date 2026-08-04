import { cn } from '@/lib/cn'

export interface AvatarProps {
  src?: string | null
  name?: string | null
  className?: string
}

/** Avatar per spec §8: round raised tile, strong hairline ring, mono
 *  initials. Reuse for user/agent avatars. */
export function Avatar({ src, name, className }: AvatarProps) {
  const cls = cn('h-7 w-7 shrink-0 rounded-full border border-line-strong', className)
  if (src) return <img src={src} alt="" className={cn(cls, 'object-cover')} />
  const words = (name ?? '').trim().split(/\s+/).filter(Boolean)
  const initials =
    words.length >= 2 ? `${words[0]![0]}${words[1]![0]}` : (words[0]?.slice(0, 2) ?? '?')
  return (
    <span
      className={cn(
        cls,
        'grid place-items-center bg-raised font-mono text-[10px] font-medium uppercase tracking-[0.05em] text-muted',
      )}
    >
      {initials}
    </span>
  )
}
