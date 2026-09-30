import { Check, Copy, Zap } from "lucide-react";
import MemoryVisualizer from "../../components/MemoryVisualizer";
import type { LaunchPreview, MemoryEstimate } from "../../bindings";

export function EstimateCard({
  estimate,
  notes,
  estimating,
  canEstimate,
  onEstimate,
}: {
  estimate: MemoryEstimate | null;
  notes: string[];
  estimating: boolean;
  canEstimate: boolean;
  onEstimate: () => void;
}) {
  return (
    <div className="card">
      <div className="flex items-center justify-between mb-3">
        <h2 className="section-title mb-0">Memory estimate</h2>
        <button
          className="btn-secondary text-xs py-1 px-2"
          onClick={onEstimate}
          disabled={estimating || !canEstimate}
          title="Estimate optimal GPU offload and cache settings for the selected model"
        >
          <Zap size={12} className={estimating ? "animate-pulse" : ""} /> Auto-estimate
        </button>
      </div>
      {estimate ? (
        <>
          <MemoryVisualizer estimate={estimate} />
          {notes.length > 0 && (
            <ul className="space-y-0.5 mt-3 pt-3 border-t border-border">
              {notes.map((n, i) => (
                <li key={i} className="text-xs text-faint">{n}</li>
              ))}
            </ul>
          )}
        </>
      ) : (
        <p className="text-xs text-dim">Select a model to estimate memory.</p>
      )}
    </div>
  );
}

export function CommandCard({
  preview,
  command,
  copied,
  onCopy,
}: {
  preview: LaunchPreview | null;
  command: string;
  copied: boolean;
  onCopy: () => void;
}) {
  return (
    <div className="card">
      <div className="flex items-center justify-between mb-1">
        <h2 className="section-title mb-0">Launch command</h2>
        {preview && (
          <button className="btn-ghost text-[0.6875rem]" onClick={onCopy} title="Copy launch command">
            {copied ? <Check size={11} className="text-accent-green" /> : <Copy size={11} />}
            {copied ? "Copied" : "Copy"}
          </button>
        )}
      </div>
      {preview ? (
        <>
          <pre className="bg-surface-0 rounded border border-border p-2 font-mono text-[0.6875rem] text-dim whitespace-pre-wrap break-all select-text">
            {command}
          </pre>
          {preview.notes
            .filter(
              (n) =>
                !n.startsWith("--mmproj") &&
                !n.startsWith("--chat-template-file") &&
                !n.startsWith("--spec-draft-model"),
            )
            .map((n, i) => (
              <p key={i} className="text-[0.6875rem] text-faint mt-1">{n}</p>
            ))}
        </>
      ) : (
        <p className="text-xs text-dim">No launch command yet.</p>
      )}
    </div>
  );
}

export function LogCard({
  logs,
  logRef,
  copied,
  onCopy,
}: {
  logs: string[];
  logRef: React.RefObject<HTMLDivElement>;
  copied: boolean;
  onCopy: () => void;
}) {
  return (
    <div className="card">
      <div className="flex items-center justify-between mb-1">
        <h2 className="section-title mb-0">Server logs</h2>
        {logs.length > 0 && (
          <button className="btn-ghost text-[0.6875rem]" onClick={onCopy} title="Copy server logs">
            {copied ? <Check size={11} className="text-accent-green" /> : <Copy size={11} />}
            {copied ? "Copied" : "Copy"}
          </button>
        )}
      </div>
      <div
        ref={logRef}
        className="bg-surface-0 rounded border border-border p-2 mt-2 h-48 overflow-y-auto font-mono text-[0.6875rem] text-dim whitespace-pre-wrap break-words"
      >
        {logs.length === 0 ? "No logs yet." : logs.join("\n")}
      </div>
    </div>
  );
}
