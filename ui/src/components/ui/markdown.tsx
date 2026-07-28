import { memo, type ReactNode } from 'react'
import ReactMarkdown, { type Components } from 'react-markdown'
import remarkGfm from 'remark-gfm'
import remarkBreaks from 'remark-breaks'
import rehypeHighlight from 'rehype-highlight'
import { AgentMediaImage } from '@/components/artifacts/save-media-image'
import { cn } from '@/lib/cn'
import { CodeBlock } from '@/components/ui/code-block'

// Full markdown for chat: GFM (tables, task lists, strikethrough, autolinks),
// soft line breaks, and syntax-highlighted fenced code. Raw HTML is NOT rendered
// (react-markdown's safe default), so no sanitization plumbing is needed.
// Mercury-styled via semantic utilities. Reuse — do not re-render markdown inline.

// ── @mention highlighting ────────────────────────────────────────────────────
// Same token shape the server notifies on (mentions.ts) plus the channel
// `:tier` suffix; requires a boundary before the @ so emails don't match.
// Matched tokens become `mention:` links, which the `a` component renders as
// a styled span — one plugin, every markdown surface at once. Text nodes
// never occur inside code/inlineCode in mdast, so code is inherently safe.
const MENTION_RE = /(^|[\s(])@([a-z0-9][a-z0-9-]*(?::[a-z0-9-]+)?)/gi

interface MdNode {
  type: string
  value?: string
  url?: string
  children?: MdNode[]
}

function remarkMentions() {
  return (tree: MdNode) => {
    const walk = (node: MdNode): void => {
      if (!node.children) return
      const next: MdNode[] = []
      for (const child of node.children) {
        if (child.type === 'text' && child.value?.includes('@')) {
          const parts: MdNode[] = []
          let last = 0
          MENTION_RE.lastIndex = 0
          for (let m = MENTION_RE.exec(child.value); m; m = MENTION_RE.exec(child.value)) {
            const start = m.index + m[1]!.length
            if (start > last) parts.push({ type: 'text', value: child.value.slice(last, start) })
            parts.push({ type: 'link', url: `mention:${m[2]}`, children: [{ type: 'text', value: `@${m[2]}` }] })
            last = start + m[2]!.length + 1
          }
          if (parts.length) {
            if (last < child.value.length) parts.push({ type: 'text', value: child.value.slice(last) })
            next.push(...parts)
            continue
          }
        }
        // Don't descend into links — a nested link is invalid mdast.
        if (child.type !== 'link') walk(child)
        next.push(child)
      }
      node.children = next
    }
    walk(tree)
  }
}

function nodeText(node: ReactNode): string {
  if (typeof node === 'string' || typeof node === 'number') return String(node)
  if (Array.isArray(node)) return node.map(nodeText).join('')
  if (node && typeof node === 'object' && 'props' in node) {
    return nodeText((node as { props: { children?: ReactNode } }).props.children)
  }
  return ''
}

const components: Partial<Components> = {
  code({ className, children, ...props }) {
    const isBlock = /language-(\w+)/.test(className ?? '')
    if (!isBlock) {
      return (
        <code className="rounded border border-line bg-card2 px-1.5 py-0.5 text-[0.85em] text-accent" {...props}>
          {children}
        </code>
      )
    }
    const language = /language-(\w+)/.exec(className ?? '')?.[1]
    return (
      <CodeBlock code={nodeText(children).replace(/\n$/, '')} language={language}>
        <code className={className} {...props}>
          {children}
        </code>
      </CodeBlock>
    )
  },
  // react-markdown wraps block code in <pre>; CodeBlock renders its own, so unwrap.
  pre: ({ children }) => <>{children}</>,
  h1: ({ children }) => <h1 className="mb-2 mt-4 text-xl font-semibold text-fg first:mt-0">{children}</h1>,
  h2: ({ children }) => <h2 className="mb-2 mt-4 text-lg font-semibold text-fg first:mt-0">{children}</h2>,
  h3: ({ children }) => <h3 className="mb-1.5 mt-3 text-base font-semibold text-fg first:mt-0">{children}</h3>,
  h4: ({ children }) => <h4 className="mb-1.5 mt-3 text-sm font-semibold text-fg first:mt-0">{children}</h4>,
  p: ({ children }) => <p className="leading-relaxed text-fg">{children}</p>,
  a: ({ children, href }) =>
    href?.startsWith('mention:') ? (
      <span className="rounded bg-[color:var(--theme-accent)]/10 px-1 font-medium text-accent">{children}</span>
    ) : (
      <a href={href} target="_blank" rel="noopener noreferrer" className="text-accent underline decoration-[var(--theme-accent-border)] underline-offset-2 hover:decoration-accent">
        {children}
      </a>
    ),
  ul: ({ children }) => <ul className="my-1 ml-5 list-disc space-y-1 marker:text-muted">{children}</ul>,
  ol: ({ children }) => <ol className="my-1 ml-5 list-decimal space-y-1 marker:text-muted">{children}</ol>,
  li: ({ children }) => <li className="leading-relaxed text-fg">{children}</li>,
  blockquote: ({ children }) => (
    <blockquote className="my-2 border-l-2 border-[var(--theme-accent-border)] pl-3 italic text-muted">{children}</blockquote>
  ),
  strong: ({ children }) => <strong className="font-semibold text-fg">{children}</strong>,
  em: ({ children }) => <em className="italic">{children}</em>,
  hr: () => <hr className="my-3 border-line-subtle" />,
  // Agent-produced images (served out of an agent container) get the
  // save-to-artifacts affordance; ordinary images render plain.
  img: ({ src, alt }) =>
    src ? (
      src.startsWith('/api/agent-media/') ? (
        <AgentMediaImage src={src} alt={alt ?? ''} />
      ) : (
        <img src={src} alt={alt ?? ''} className="my-2 max-h-96 rounded-lg border border-line" />
      )
    ) : null,
  table: ({ children }) => (
    <div className="my-3 max-w-full overflow-x-auto rounded-lg border border-line">
      <table className="w-full border-collapse text-sm">{children}</table>
    </div>
  ),
  thead: ({ children }) => <thead className="border-b border-line bg-card2">{children}</thead>,
  th: ({ children }) => <th className="px-3 py-2 text-left font-semibold text-fg">{children}</th>,
  td: ({ children }) => <td className="border-t border-line-subtle px-3 py-2 align-top text-fg">{children}</td>,
}

export const Markdown = memo(function Markdown({ children, className }: { children: string; className?: string }) {
  return (
    // Rendered prose is a READING surface — sans (code re-asserts mono).
    <div className={cn('space-y-2 break-words font-sans', className)}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm, remarkBreaks, remarkMentions]}
        rehypePlugins={[[rehypeHighlight, { detect: true, ignoreMissing: true }]]}
        components={components}
      >
        {children}
      </ReactMarkdown>
    </div>
  )
})
