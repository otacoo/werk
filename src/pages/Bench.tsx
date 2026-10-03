import { useEffect, useState } from "react";
import { FlaskConical, RefreshCw, Trash2 } from "lucide-react";
import { commands } from "../bindings";
import type { BenchResult } from "../bindings";
import { call } from "../utils/ipc";

export default function Bench() {
  const [results, setResults] = useState<BenchResult[]>([]);
  const [loading, setLoading] = useState(false);
  const [model, setModel] = useState("");
  const [ngl, setNgl] = useState("");
  const [running, setRunning] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const load = async () => {
    setLoading(true);
    try {
      const r = await call(commands.benchHistory());
      setResults([...r].reverse());
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    load();
    call(commands.getConfig())
      .then((c) => setModel(c.selected_model ?? ""))
      .catch(() => {});
  }, []);

  const run = async () => {
    if (!model) {
      setError("Select a model in Run first.");
      return;
    }
    setError(null);
    setRunning(true);
    try {
      const parsed = parseInt(ngl, 10);
      const layers = ngl.trim() === "" || Number.isNaN(parsed) ? null : parsed;
      await call(commands.runBench(model, layers));
      await load();
    } catch (e) {
      setError(String(e));
    } finally {
      setRunning(false);
    }
  };

  const clear = async () => {
    if (!window.confirm("Clear all bench history?")) return;
    try {
      await call(commands.clearBenchHistory());
      setResults([]);
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div className="h-full overflow-y-auto">
      <div className="p-6 space-y-4">
        <div className="flex items-center justify-between gap-2">
          <div>
            <h1 className="section-title flex items-center gap-2">
              <FlaskConical size={16} className="text-accent-soft" /> Bench
            </h1>
            <p className="section-desc">
              Quick Bench runs one rep of pp512 + tg128 with llama-bench on the selected
              model. Higher tg means faster generation, higher pp faster prefill — compare
              rows before and after changing a knob.
            </p>
          </div>
          <div className="flex gap-2 shrink-0">
            <button className="btn-ghost text-xs py-1 px-2" onClick={load} disabled={loading}>
              <RefreshCw size={12} className={loading ? "animate-spin" : ""} /> Refresh
            </button>
            <button
              className="btn-ghost text-xs py-1 px-2 text-accent-red"
              onClick={clear}
              disabled={results.length === 0}
            >
              <Trash2 size={12} /> Clear
            </button>
          </div>
        </div>

        <div className="card">
          <h2 className="section-title mb-1">Quick Bench</h2>
          <p className="section-desc">Runs against the model selected in Run.</p>
          <div className="flex flex-wrap items-center gap-3 mt-2">
            <span className="text-xs text-ink truncate max-w-[50%]" title={model}>
              {model.split(/[\\/]/).pop() || "No model selected"}
            </span>
            <label
              className="flex items-center gap-1.5 text-[0.6875rem] text-faint"
              title="How many layers of the model run on the GPU; the rest stay on the CPU. Empty = auto, -1 = all layers, 0 = CPU only."
            >
              <span>GPU</span>
              <input
                type="number"
                min={-1}
                placeholder="auto"
                className="input w-16 py-0.5 px-1.5 text-[0.6875rem]"
                value={ngl}
                onChange={(e) => setNgl(e.target.value)}
              />
            </label>
            <button
              className="btn-primary text-xs ml-auto"
              disabled={running || !model}
              onClick={run}
            >
              {running ? "Running…" : "Run benchmark"}
            </button>
          </div>
        </div>

        {error && (
          <div className="card border-accent-red/30 bg-accent-red/5 text-sm text-accent-red">{error}</div>
        )}

        {results.length === 0 ? (
          <div className="card">
            <p className="text-sm text-dim">No bench results yet. Run one above.</p>
          </div>
        ) : (
          <div className="card p-0 overflow-hidden">
            <div className="overflow-x-auto">
              <table className="w-full text-xs">
                <thead className="bg-surface-1 text-dim">
                  <tr>
                    <th className="text-left px-3 py-2 font-medium">Time</th>
                    <th className="text-left px-3 py-2 font-medium">Model</th>
                    <th className="text-right px-3 py-2 font-medium">pp t/s</th>
                    <th className="text-right px-3 py-2 font-medium">tg t/s</th>
                    <th className="text-center px-3 py-2 font-medium">GPU layers</th>
                    <th className="text-left px-3 py-2 font-medium">Status</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-border">
                  {results.map((r, i) => (
                    <tr key={`${r.timestamp}-${i}`} className="hover:bg-surface-1">
                      <td className="px-3 py-2 text-dim font-mono whitespace-nowrap">
                        {new Date(r.timestamp * 1000).toLocaleString()}
                      </td>
                      <td className="px-3 py-2 text-ink truncate max-w-[220px]" title={r.model_path}>
                        {r.model_name || r.model_path.split(/[\\/]/).pop() || "–"}
                      </td>
                      <td className="px-3 py-2 text-right text-ink">{r.pp_tps?.toFixed(1) ?? "–"}</td>
                      <td className="px-3 py-2 text-right text-ink">{r.tg_tps?.toFixed(1) ?? "–"}</td>
                      <td className="px-3 py-2 text-center text-dim">{r.n_gpu_layers}</td>
                      <td className="px-3 py-2 text-dim truncate max-w-[200px]" title={r.status}>
                        {r.status}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
