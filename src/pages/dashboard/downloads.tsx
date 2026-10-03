import { fmtMB } from "../../utils/ipc";
import { baseOf, type ActiveDl } from "./shared";

export function DownloadRow({ prog, paused, onPause, onResume, onCancel }: {
  prog?: { downloaded: number; total: number | null };
  paused: boolean;
  onPause: () => void;
  onResume: () => void;
  onCancel: () => void;
}) {
  return (
    <div className="mt-2">
      <div className="flex justify-between items-center text-xs text-dim mb-1">
        <span>{paused ? "Paused" : "Downloading…"}</span>
        <span>
          {prog?.total ? `${((prog.downloaded / prog.total) * 100).toFixed(1)}%` : fmtMB((prog?.downloaded ?? 0) / 1024 / 1024)}
        </span>
      </div>
      <div className="h-1.5 bg-surface-3 overflow-hidden">
        <div
          className="h-full bg-accent transition-all"
          style={{ width: prog?.total ? `${(prog.downloaded / prog.total) * 100}%` : "100%" }}
        />
      </div>
      <div className="flex gap-2 mt-2">
        {paused ? (
          <>
            <button className="btn-primary text-xs" onClick={onResume}>
              Resume
            </button>
            <button className="btn-danger text-xs" onClick={onCancel}>
              Cancel
            </button>
          </>
        ) : (
          <>
            <button className="btn-ghost text-xs" onClick={onPause}>
              Pause
            </button>
            <button className="btn-danger text-xs" onClick={onCancel}>
              Cancel
            </button>
          </>
        )}
      </div>
    </div>
  );
}

export function ActiveDownloads({ active, progress, paused, onPause, onResume, onCancel }: {
  active: Record<string, ActiveDl>;
  progress: Record<string, { downloaded: number; total: number | null }>;
  paused: Record<string, boolean>;
  onPause: (id: string) => void;
  onResume: (id: string) => void;
  onCancel: (id: string) => void;
}) {
  const ids = Object.keys(active);
  if (ids.length === 0) return null;
  return (
    <div className="mb-2 rounded border border-border bg-surface-2 p-4">
      <h3 className="text-xs font-medium text-dim mb-2">Downloading</h3>
      <div className="space-y-3">
        {ids.map((id) => {
          const dl = progress[id];
          const isPaused = !!paused[id];
          return (
            <div key={id}>
              <div className="flex items-center justify-between mb-1 gap-2">
                <span className="text-xs text-ink truncate mr-3 font-mono">{baseOf(id)}</span>
                <div className="flex items-center gap-2 shrink-0">
                  {isPaused ? (
                    <>
                      <span className="text-[0.625rem] text-accent-yellow">Paused</span>
                      <button className="btn-primary text-[0.625rem] py-0.5 px-1.5" onClick={() => onResume(id)}>
                        Resume
                      </button>
                      <button
                        className="btn-danger text-[0.625rem] py-0.5 px-1.5"
                        onClick={() => onCancel(id)}
                        title="Discard the partial download"
                      >
                        Cancel
                      </button>
                    </>
                  ) : (
                    <>
                      <span className="text-[0.625rem] font-mono text-dim">
                        {dl?.total ? `${((dl.downloaded / dl.total) * 100).toFixed(1)}%` : fmtMB((dl?.downloaded ?? 0) / 1024 / 1024)}
                      </span>
                      <button className="btn-ghost text-[0.625rem] py-0.5 px-1.5" onClick={() => onPause(id)}>
                        Pause
                      </button>
                      <button
                        className="btn-danger text-[0.625rem] py-0.5 px-1.5"
                        onClick={() => onCancel(id)}
                        title="Discard the partial download"
                      >
                        Cancel
                      </button>
                    </>
                  )}
                </div>
              </div>
              {!isPaused && (
                <div className="h-1 bg-surface-3 overflow-hidden">
                  <div
                    className="h-full bg-accent transition-all"
                    style={{ width: dl?.total ? `${(dl.downloaded / dl.total) * 100}%` : "100%" }}
                  />
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

