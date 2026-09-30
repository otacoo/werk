import { useCallback, useEffect, useState } from "react";
import { Check, Copy, Database, RefreshCw, Server } from "lucide-react";
import { commands } from "../bindings";
import type { ServerInfo, ServerStatus } from "../bindings";
import { call } from "../utils/ipc";

function CopyRow({ label, value, mono = false }: { label: string; value: string; mono?: boolean }) {
  const [copied, setCopied] = useState(false);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {}
  };

  return (
    <div className="flex items-center gap-3 px-3 py-2 border border-border bg-surface-1 rounded">
      <span className="text-xs text-dim w-36 shrink-0">{label}</span>
      <span className={`flex-1 text-sm text-ink truncate ${mono ? "font-mono text-xs" : ""}`}>
        {value || "—"}
      </span>
      <button
        className="text-faint hover:text-ink transition-colors shrink-0"
        onClick={copy}
        title="Copy to clipboard"
      >
        {copied ? <Check size={14} className="text-accent-green" /> : <Copy size={14} />}
      </button>
    </div>
  );
}

// API section card (lives on the Tools page).
export default function ApiCard() {
  const [status, setStatus] = useState<ServerStatus>({ type: "stopped" });
  const [info, setInfo] = useState<ServerInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);

  const loadInfo = useCallback(async () => {
    setRefreshing(true);
    setError(null);
    try {
      setInfo(await call(commands.getServerInfo()));
    } catch (e) {
      setInfo(null);
      setError(String(e));
    } finally {
      setRefreshing(false);
    }
  }, []);

  useEffect(() => {
    const poll = async () => {
      try {
        setStatus(await call(commands.getServerStatus()));
      } catch {}
    };
    poll();
    const id = setInterval(poll, 2000);
    return () => clearInterval(id);
  }, []);

  useEffect(() => {
    if (status.type === "running") {
      loadInfo();
      const id = setInterval(loadInfo, 5000);
      return () => clearInterval(id);
    }
    setInfo(null);
    setError(null);
  }, [status.type === "running" ? "running" : "not", loadInfo]);

  if (status.type !== "running") {
    return (
      <div className="card flex items-center gap-3 text-dim">
        <Server size={16} className="text-faint" />
        <p className="text-xs">API details appear when the server is running.</p>
      </div>
    );
  }

  const apiKeyLine = info?.api_key ? `\n  "apiKey": "${info.api_key}",` : "";
  const envConfig = info
    ? `OPENAI_BASE_URL=${info.base_url}${info.api_key ? `\nOPENAI_API_KEY=${info.api_key}` : ""}\nOPENAI_MODEL=${info.model_id}`
    : "";

  return (
    <div className="card">
      <div className="flex items-center justify-between">
        <h2 className="section-title mb-0">API</h2>
        <button className="btn-ghost text-xs py-1 px-2" onClick={loadInfo} disabled={refreshing}>
          <RefreshCw size={12} className={refreshing ? "animate-spin" : ""} />
          Refresh
        </button>
      </div>
      <p className="section-desc">Connection details for the running server — OpenAI-compatible.</p>

      {error && (
        <div className="card border-accent-red/30 bg-accent-red/5 mt-2">
          <p className="text-sm text-accent-red">{error}</p>
        </div>
      )}

      <div className="space-y-4 mt-3">
        <div>
          <h3 className="text-xs font-semibold text-dim uppercase tracking-wider mb-2">Endpoints</h3>
          <div className="space-y-1.5">
            <CopyRow label="Base URL" value={info?.base_url ?? ""} mono />
            <CopyRow label="Chat" value={`${info?.base_url ?? ""}/chat/completions`} mono />
            <CopyRow label="Completions" value={`${info?.base_url ?? ""}/completions`} mono />
            <CopyRow label="Embeddings" value={`${info?.base_url ?? ""}/embeddings`} mono />
            <CopyRow label="List models" value={`${info?.base_url ?? ""}/models`} mono />
            <CopyRow label="API key" value={info?.api_key ?? "(none — open server)"} mono />
          </div>
        </div>

        <div>
          <h3 className="text-xs font-semibold text-dim uppercase tracking-wider mb-2">
            {info?.router_mode ? "Serving Model (router)" : "Loaded Model"}
          </h3>
          {info?.router_mode && !info.model_id && (
            <p className="text-xs text-dim mb-2">No model loaded — the router loads on demand.</p>
          )}
          <div className="space-y-1.5">
            <CopyRow label="Model ID" value={info?.model_id ?? ""} mono />
            {!info?.router_mode && <CopyRow label="Alias" value={info?.model_alias ?? ""} mono />}
            <CopyRow label="Path" value={info?.model_path ?? ""} mono />
            <CopyRow label="Context (n_ctx)" value={info ? String(info.n_ctx) : ""} mono />
            {!info?.router_mode && (
              <>
                <CopyRow label="Max tokens" value={info ? String(info.n_predict) : ""} mono />
                <CopyRow
                  label="Slots"
                  value={info ? `${info.slots_idle} idle / ${info.total_slots} total` : ""}
                />
              </>
            )}
          </div>
        </div>

        {info?.router_mode && (info.models ?? []).length > 0 && (
          <div>
            <h3 className="text-xs font-semibold text-dim uppercase tracking-wider mb-2">Registered Models</h3>
            <div className="space-y-1.5">
              {info.models!.map((m) => (
                <div
                  key={m.id}
                  className="flex items-center gap-3 px-3 py-2 border border-border bg-surface-1 rounded"
                >
                  <span className="flex-1 text-xs text-ink font-mono truncate">{m.id}</span>
                  <span
                    className={`text-[0.625rem] shrink-0 ${m.status === "loaded" ? "text-accent-green" : "text-dim"}`}
                  >
                    {m.status}
                  </span>
                </div>
              ))}
            </div>
          </div>
        )}

        {info && (
          <div>
            <h3 className="text-xs font-semibold text-dim uppercase tracking-wider mb-2">Client Configuration</h3>
            <p className="text-xs text-faint mb-3">
              Environment variables for OpenAI-compatible SDKs (Python, Node, curl…).
            </p>
            <div className="flex items-center gap-3">
              <pre className="flex-1 bg-surface-0 border border-border rounded p-3 font-mono text-xs text-ink overflow-x-auto select-text whitespace-pre-wrap">
                {envConfig}
              </pre>
              <button
                className="text-faint hover:text-ink transition-colors shrink-0"
                onClick={async () => {
                  try {
                    await navigator.clipboard.writeText(envConfig);
                  } catch {}
                }}
                title="Copy environment variables"
              >
                <Copy size={14} />
              </button>
            </div>
            {apiKeyLine && (
              <p className="text-xs text-faint mt-2">
                <Database size={11} className="inline mr-1" />
                The server was started with an API key — include it in all requests.
              </p>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
