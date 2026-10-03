import { useRef, useState } from "react";
import type { ReactNode } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import ReactMarkdown from "react-markdown";
import type { Components } from "react-markdown";
import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import remarkBreaks from "remark-breaks";
import rehypeKatex from "rehype-katex";
import rehypeHighlight from "rehype-highlight";
import type { PluggableList } from "unified";
import "katex/dist/katex.min.css";

const REMARK_PLUGINS = [remarkGfm, remarkMath, remarkBreaks];
// `detect: false` only colors fenced blocks that name a language; unknown
// languages pass through untouched.
const REHYPE_PLUGINS: PluggableList = [
  rehypeKatex,
  [rehypeHighlight, { detect: false, ignoreMissing: true }],
];

// Code blocks get a copy button on hover; inline code renders untouched.
function CodeBlock({ children }: { children?: ReactNode }) {
  const ref = useRef<HTMLPreElement>(null);
  const [copied, setCopied] = useState(false);
  return (
    <div className="relative group/code">
      <pre ref={ref}>{children}</pre>
      <button
        className="absolute top-1.5 right-1.5 text-[0.625rem] text-dim hover:text-ink bg-surface-2 border border-border rounded px-1.5 py-0.5 opacity-0 group-hover/code:opacity-100 transition-opacity"
        onClick={async () => {
          const text = ref.current?.textContent ?? "";
          try {
            await navigator.clipboard.writeText(text);
            setCopied(true);
            setTimeout(() => setCopied(false), 1500);
          } catch {}
        }}
        title="Copy code"
      >
        {copied ? "Copied" : "Copy"}
      </button>
    </div>
  );
}

// Stable component overrides: an inline object would remount code blocks on
// every streamed delta, making the copy button flicker mid-hover.
const MD_COMPONENTS: Components = {
  pre: (props) => <CodeBlock>{props.children}</CodeBlock>,
  a: ({ href, children }) => (
    <button
      className="text-accent-soft underline break-all"
      onClick={() => {
        if (href) openUrl(href).catch(() => {});
      }}
    >
      {children}
    </button>
  ),
};

export function Markdown({ content }: { content: string }) {
  return (
    <div className="md select-text">
      <ReactMarkdown
        remarkPlugins={REMARK_PLUGINS}
        rehypePlugins={REHYPE_PLUGINS}
        components={MD_COMPONENTS}
      >
        {content}
      </ReactMarkdown>
    </div>
  );
}
