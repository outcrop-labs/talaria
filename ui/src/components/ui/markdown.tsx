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
  a: ({ children, href }) => (
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
    <div className={cn('space-y-2 break-words', className)}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm, remarkBreaks]}
        rehypePlugins={[[rehypeHighlight, { detect: true, ignoreMissing: true }]]}
        components={components}
      >
        {children}
      </ReactMarkdown>
    </div>
  )
})
