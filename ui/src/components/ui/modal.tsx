import { useEffect } from 'react'
import { createPortal } from 'react-dom'
import { motion, AnimatePresence } from 'framer-motion'
import type { ReactNode } from 'react'
import { CloseButton } from './close-button'

// The one modal. Esc + backdrop-click close. Two shapes:
//   • centered panel (default) — confirms, single-field creates, pickers.
//   • `takeover` — fills the screen minus a padding gutter (the gutter is the
//     "you're in a dialog" cue), constant height, content scrolls inside.
//     Use for anything substantial: tabbed managers, libraries, composers.
//
// Portaled to <body>: `position: fixed` is relative to the nearest ancestor
// with a transform/filter/backdrop-filter (the mercury-panel surfaces have
// backdrop-filter), which would otherwise center the modal inside a card rather
// than the viewport. The portal escapes any such containing block.
export function Modal({
  open,
  onClose,
  title,
  children,
  footer,
  width = 'max-w-md',
  takeover = false,
}: {
  open: boolean
  onClose: () => void
  title?: ReactNode
  children: ReactNode
  footer?: ReactNode
  width?: string
  takeover?: boolean
}) {
  useEffect(() => {
    if (!open) return
    const onKey = (e: KeyboardEvent) => e.key === 'Escape' && onClose()
    document.addEventListener('keydown', onKey)
    return () => document.removeEventListener('keydown', onKey)
  }, [open, onClose])

  if (typeof document === 'undefined') return null

  return createPortal(
    <AnimatePresence>
      {open && (
        <div className={takeover ? 'fixed inset-0 z-50 p-6 sm:p-8' : 'fixed inset-0 z-50 grid place-items-center p-4'}>
          <motion.div
            className="absolute inset-0 bg-black/50"
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            onClick={onClose}
          />
          <motion.div
            role="dialog"
            aria-modal
            initial={{ opacity: 0, y: 12, scale: 0.98 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 12, scale: 0.98 }}
            transition={{ duration: 0.16 }}
            className={
              takeover
                ? 'mercury-panel relative z-10 flex h-full w-full flex-col overflow-hidden rounded-2xl'
                : `mercury-panel relative z-10 w-full ${width} rounded-2xl`
            }
          >
            {title && (
              <div className="flex shrink-0 items-center justify-between border-b border-line-subtle px-7 py-4">
                <div className="text-sm font-semibold text-fg">{title}</div>
                <CloseButton onClick={onClose} className="-mr-2" />
              </div>
            )}
            <div className={takeover ? 'min-h-0 flex-1 overflow-y-auto p-7' : 'p-7'}>{children}</div>
            {footer && <div className="shrink-0 border-t border-line-subtle px-7 py-4">{footer}</div>}
          </motion.div>
        </div>
      )}
    </AnimatePresence>,
    document.body,
  )
}
