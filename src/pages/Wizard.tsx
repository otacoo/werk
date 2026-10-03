import { useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { getCurrentWindow } from "@tauri-apps/api/window";
import {
  ChevronLeft,
  ChevronRight,
  CircleCheck,
  Cpu,
  Download,
  FolderOpen,
  MemoryStick,
  Monitor,
  RefreshCw,
  Zap,
} from "lucide-react";
import WindowControls from "../components/WindowControls";
import { THEME_OPTIONS, ThemeIcon } from "../components/ThemeIcon";
import { commands } from "../bindings";
import type {
  AssetDto,
  FoundBinaryDto,
  RecommendedDto,
  ReleaseDto,
  RuntimeInfo,
  SystemInfoDto,
} from "../bindings";
import { call, fmtMB } from "../utils/ipc";
import {
  accentPalette,
  getAccent,
  getCorners,
  getTheme,
  setAccent,
  setCorners,
  setTheme,
  subscribeAppearance,
  type AccentName,
  type CornersMode,
} from "../utils/appearance";
import { mbToGb, shortCpuName, shortGpuName } from "../utils/format";

type Step = 1 | 2 | 3;
type Progress = { downloaded: number; total: number | null };
type FitLevel = "vram" | "mixed" | "tight" | "no";

const FIT_ORDER: Record<FitLevel, number> = { vram: 0, mixed: 1, tight: 2, no: 3 };

const STEPS: { n: Step; label: string }[] = [
  { n: 1, label: "System & Runtime" },
  { n: 2, label: "Models" },
  { n: 3, label: "Appearance" },
];

const FIT_BADGE: Record<FitLevel, { text: string; cls: string }> = {
  vram: { text: "Fits in VRAM", cls: "badge-green" },
  mixed: { text: "VRAM + RAM", cls: "badge-blue" },
  tight: { text: "Tight fit", cls: "badge-yellow" },
  no: { text: "Too large", cls: "badge-gray" },
};

function fitFor(sizeMb: number, vramMb: number, ramMb: number): FitLevel {
  const memory = vramMb + ramMb;
  if (vramMb > 0 && sizeMb < vramMb * 0.85) return "vram";
  if (sizeMb < memory * 0.7) return "mixed";
  if (sizeMb < memory * 0.9) return "tight";
  return "no";
}

function progressText(p: Progress) {
  if (!p.total) return fmtMB(p.downloaded / 1024 / 1024);
  return `${((p.downloaded / p.total) * 100).toFixed(1)}% — ${fmtMB(p.downloaded / 1024 / 1024)} / ${fmtMB(p.total / 1024 / 1024)}`;
}

function Bar({ progress }: { progress: Progress }) {
  const width = progress.total ? Math.min(100, (progress.downloaded / progress.total) * 100) : 100;
  return (
    <div className="h-1.5 bg-surface-3 overflow-hidden">
      <div className="h-full bg-accent transition-all" style={{ width: `${width}%` }} />
    </div>
  );
}

function useAppearance() {
  const [snap, setSnap] = useState({ theme: getTheme(), accent: getAccent(), corners: getCorners() });
  useEffect(
    () =>
      subscribeAppearance(() =>
        setSnap({ theme: getTheme(), accent: getAccent(), corners: getCorners() }),
      ),
    [],
  );
  return snap;
}

export default function Wizard({ onDone }: { onDone: () => void }) {
  const [step, setStep] = useState<Step>(1);
  const [system, setSystem] = useState<SystemInfoDto | null>(null);
  const { theme, accent, corners } = useAppearance();

  const [runtime, setRuntime] = useState<RuntimeInfo | null>(null);
  const [runtimeInstalled, setRuntimeInstalled] = useState(false);
  const [release, setRelease] = useState<ReleaseDto | null>(null);
  const [checked, setChecked] = useState(false);
  const [checking, setChecking] = useState(false);
  const [asset, setAsset] = useState<string | null>(null);
  const [rtProgress, setRtProgress] = useState<Progress | null>(null);
  const [rtError, setRtError] = useState<string | null>(null);
  const [scanning, setScanning] = useState(false);
  const [foundBuilds, setFoundBuilds] = useState<FoundBinaryDto[] | null>(null);
  const rtNote = useRef<string | null>(null);

  const [models, setModels] = useState<RecommendedDto[]>([]);
  const [picks, setPicks] = useState<Set<string>>(new Set());
  const [dlProgress, setDlProgress] = useState<Record<string, Progress>>({});
  const [installed, setInstalled] = useState<Set<string>>(new Set());
  const [locations, setLocations] = useState<Record<string, string>>({});
  const [modelErrors, setModelErrors] = useState<Record<string, string>>({});
  const [queue, setQueue] = useState(false);
  const [modelsDir, setModelsDir] = useState<string | null>(null);
  const [dirError, setDirError] = useState<string | null>(null);
  const book = useRef(new Map<string, { repoPath: string; stopNote: string | null }>());

  // Progress events arrive in chunks; repaint at most ~8fps.
  const lastTick = useRef(0);

  useEffect(() => {
    call(commands.getSystemInfo()).then(setSystem).catch(() => {});
    call(commands.getRuntimeInfo())
      .then((rt) => {
        setRuntime(rt);
        if (rt.server_binary) setRuntimeInstalled(true);
      })
      .catch(() => {});
    call(commands.recommendedModels()).then(setModels).catch(() => {});
    call(commands.getModelDirs())
      .then((d) => setModelsDir(d.download_dir))
      .catch(() => {});
    const unlisten = listen<{ id: string; downloaded: number; total?: number | null }>(
      "download_progress",
      (e) => {
        const now = Date.now();
        if (now - lastTick.current < 120) return;
        lastTick.current = now;
        const p = e.payload;
        if (p.id.startsWith("runtime:")) {
          setRtProgress({ downloaded: p.downloaded, total: p.total ?? null });
        } else {
          const name = p.id.split("/").pop() ?? p.id;
          setDlProgress((prev) => ({
            ...prev,
            [name]: { downloaded: p.downloaded, total: p.total ?? null },
          }));
        }
      },
    );
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  const vram = system?.gpus.reduce((s, g) => s + g.vram_mb, 0) ?? 0;
  const ram = system?.available_ram_mb ?? 0;
  const cuda = system?.backends.find((b) => b.id === "cuda" && b.available);
  const runtimeReady = runtimeInstalled || runtime?.server_binary != null;

  const finish = async () => {
    try {
      await call(commands.setWizardCompleted(true));
    } catch {
      return;
    }
    onDone();
  };

  const probeBuilds = async () => {
    setChecking(true);
    setRtError(null);
    try {
      const rel = await call(commands.checkRelease());
      setRelease(rel);
      if (rel.assets.length > 0) setAsset(rel.assets[0].name);
      setChecked(true);
    } catch (e) {
      setRtError(String(e));
    } finally {
      setChecking(false);
    }
  };

  const installRuntime = async () => {
    if (!asset) return;
    setRtError(null);
    setRtProgress({ downloaded: 0, total: null });
    rtNote.current = null;
    try {
      await call(commands.downloadRuntimeAsset(asset));
      setRuntime(await call(commands.getRuntimeInfo()));
      setRuntimeInstalled(true);
    } catch (e) {
      const msg = String(e);
      const note = rtNote.current;
      rtNote.current = null;
      setRtError(note ?? (/cancel/i.test(msg) ? "Download cancelled — press Download to resume." : msg));
    } finally {
      setRtProgress(null);
    }
  };

  const haltRuntime = async (note: string) => {
    if (!asset) return;
    rtNote.current = note;
    try {
      await call(commands.cancelDownload(`runtime:${asset}`));
    } catch {}
  };

  const locateRuntime = async () => {
    let dir: string | null = null;
    try {
      const picked = await open({ directory: true, title: "Select llama.cpp directory" });
      if (typeof picked !== "string" || !picked) return;
      dir = picked;
    } catch (e) {
      setRtError(String(e));
      return;
    }
    setScanning(true);
    setRtError(null);
    try {
      const builds = await call(commands.scanCustomBinaries(dir));
      if (builds.length === 0) {
        setRtError("No llama-server binary found in the selected directory.");
      } else if (builds.length === 1) {
        await adoptBuild(builds[0].binary_path);
      } else {
        setFoundBuilds(builds);
      }
    } catch (e) {
      setRtError(String(e));
    } finally {
      setScanning(false);
    }
  };

  const adoptBuild = async (binaryPath: string) => {
    try {
      await call(commands.addCustomRuntime(binaryPath));
      setRuntime(await call(commands.getRuntimeInfo()));
      setRuntimeInstalled(true);
      setFoundBuilds(null);
    } catch (e) {
      setRtError(String(e));
    }
  };

  const togglePick = (filename: string) => {
    setPicks((prev) => {
      const next = new Set(prev);
      if (next.has(filename)) next.delete(filename);
      else if (next.size < 3) next.add(filename);
      return next;
    });
  };

  const downloadPicks = async () => {
    const targets = models.filter(
      (m) => picks.has(m.filename) && !m.installed && !installed.has(m.filename),
    );
    setQueue(true);
    try {
      for (const m of targets) {
        setModelErrors(({ [m.filename]: _, ...rest }) => rest);
        try {
          const files = await call(commands.getHfRepoFiles(m.repo_id));
          const repoPath = files
            .map((f) => f.path)
            .find((f) => f === m.filename || f.endsWith("/" + m.filename));
          if (!repoPath) {
            setModelErrors((prev) => ({ ...prev, [m.filename]: "File not found in repo" }));
            continue;
          }
          book.current.set(m.filename, { repoPath, stopNote: null });
          const saved = await call(commands.downloadModel(m.repo_id, repoPath, null, null));
          setInstalled((prev) => new Set(prev).add(m.filename));
          setLocations((prev) => ({ ...prev, [m.filename]: saved }));
          setDlProgress(({ [m.filename]: _, ...rest }) => rest);
        } catch (e) {
          const msg = String(e);
          const note = book.current.get(m.filename)?.stopNote ?? null;
          const stop = note ?? (/cancel/i.test(msg) ? "Cancelled — press Download to resume." : null);
          setModelErrors((prev) => ({ ...prev, [m.filename]: stop ?? msg }));
          setDlProgress(({ [m.filename]: _, ...rest }) => rest);
          if (stop) break;
        }
      }
      call(commands.recommendedModels()).then(setModels).catch(() => {});
    } finally {
      setQueue(false);
    }
  };

  const haltModel = async (filename: string, note: string) => {
    const entry = book.current.get(filename);
    if (!entry) return;
    entry.stopNote = note;
    try {
      await call(commands.cancelDownload(entry.repoPath));
    } catch {}
  };

  const changeModelsDir = async () => {
    setDirError(null);
    try {
      const picked = await open({ directory: true, title: "Models folder" });
      if (typeof picked !== "string" || !picked) return;
      await call(commands.setDownloadDir(picked));
      const d = await call(commands.getModelDirs());
      setModelsDir(d.download_dir);
      call(commands.recommendedModels()).then(setModels).catch(() => {});
    } catch (e) {
      setDirError(String(e));
    }
  };

  const ordered = useMemo(
    () =>
      [...models].sort((a, b) => {
        const fa = fitFor(a.estimated_size_mb, vram, ram);
        const fb = fitFor(b.estimated_size_mb, vram, ram);
        return FIT_ORDER[fa] - FIT_ORDER[fb] || a.estimated_size_mb - b.estimated_size_mb;
      }),
    [models, vram, ram],
  );

  const picksDone =
    picks.size > 0 &&
    [...picks].every(
      (f) => installed.has(f) || models.find((m) => m.filename === f)?.installed,
    );

  const builds = release?.assets.slice(0, 5) ?? [];

  const stepHint =
    step === 1
      ? runtimeReady
        ? "Runtime ready."
        : "Install a llama.cpp build or point to an existing one."
      : step === 2
        ? `${picks.size}/3 selected`
        : "Changes preview live.";

  return (
    <div className="h-screen overflow-hidden bg-surface-0 text-ink flex flex-col border border-border">
      <div className="relative flex items-stretch h-14 shrink-0 border-b border-border select-none">
        <div
          data-tauri-drag-region
          className="absolute inset-0"
          onDoubleClick={() => {
            getCurrentWindow().toggleMaximize().catch((e) => console.error("Maximize failed:", e));
          }}
        />
        <div className="relative z-10 flex flex-1 items-center gap-4 px-4 min-w-0">
          <p className="flex items-baseline gap-1.5 shrink-0 text-sm font-semibold select-none">
            werk<span className="text-accent text-[1.1em]">.</span>
            <span className="text-sm font-normal text-dim">Setup</span>
          </p>
          <nav className="flex items-center gap-0.5 ml-auto min-w-0 overflow-x-auto">
            {STEPS.map(({ n, label }) => {
              const active = step === n;
              const done = step > n;
              return (
                <button
                  key={n}
                  onClick={() => setStep(n)}
                  className={`flex items-center gap-1.5 px-2.5 py-1 rounded text-xs font-medium whitespace-nowrap transition-colors ${
                    active ? "bg-accent/20 text-ink" : "text-dim hover:text-ink hover:bg-accent/10"
                  }`}
                >
                  <span
                    className={`w-4 h-4 rounded-full text-[0.625rem] flex items-center justify-center shrink-0 ${
                      active
                        ? "bg-accent text-white"
                        : done
                          ? "border border-accent/50 text-accent-soft"
                          : "border border-border text-faint"
                    }`}
                  >
                    {n}
                  </span>
                  {label}
                </button>
              );
            })}
          </nav>
        </div>
        <div className="relative z-10 shrink-0">
          <WindowControls tall />
        </div>
      </div>

      <div className="flex-1 min-h-0 overflow-y-auto px-6 py-6">
        {step === 1 && (
          <div className="max-w-3xl mx-auto space-y-5">
            <div>
              <h2 className="section-title">System &amp; Runtime</h2>
              <p className="section-desc">
                We detected your hardware. Download a llama.cpp build or point to an existing
                installation.
              </p>
            </div>

            {scanning && (
              <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/50">
                <div className="card flex items-center gap-3 px-6 py-4">
                  <RefreshCw size={16} className="text-accent animate-spin" />
                  <span className="text-sm text-ink">Searching for server runtimes…</span>
                </div>
              </div>
            )}

            {rtError && (
              <div className="card border-accent-red/30 bg-accent-red/5">
                <p className="text-sm text-accent-red">{rtError}</p>
              </div>
            )}

            {system && (
              <div className="grid grid-cols-2 gap-2.5 sm:grid-cols-4">
                <StatTile
                  icon={Cpu}
                  label="CPU"
                  value={shortCpuName(system.cpu_name)}
                  detail={`${system.cpu_cores} cores · ${system.cpu_threads} threads`}
                />
                <StatTile
                  icon={MemoryStick}
                  label="Memory"
                  value={mbToGb(system.total_ram_mb)}
                  detail={`${mbToGb(system.available_ram_mb)} free`}
                />
                <StatTile
                  icon={Monitor}
                  label="GPU"
                  value={system.gpus.length > 0 ? shortGpuName(system.gpus[0].name) : "None"}
                  detail={
                    vram > 0
                      ? `${mbToGb(vram)} VRAM`
                      : system.gpus.length > 0
                        ? "shared memory"
                        : "No GPU detected"
                  }
                />
                <StatTile
                  icon={Zap}
                  label="Backend"
                  value={`${system.recommended_backend.toUpperCase()}${cuda?.version ? ` ${cuda.version}` : ""}`}
                  detail="Recommended for this machine"
                />
              </div>
            )}

            {runtimeReady && runtime?.server_binary && (
              <div className="card border-accent-green/30 bg-accent-green/5">
                <div className="flex items-center gap-2">
                  <CircleCheck size={16} className="text-accent-green" />
                  <span className="text-sm text-ink">Runtime ready</span>
                </div>
                <p className="text-xs text-dim mt-1 font-mono break-all">{runtime.server_binary}</p>
              </div>
            )}

            {foundBuilds && foundBuilds.length > 1 && (
              <div className="card">
                <h3 className="text-sm font-semibold text-ink mb-2">Multiple builds found</h3>
                <div className="space-y-1.5">
                  {foundBuilds.map((b) => (
                    <button
                      key={b.binary_path}
                      className="w-full flex items-center gap-3 px-3 py-2 border rounded border-border hover:bg-surface-2 text-left transition-colors"
                      onClick={() => adoptBuild(b.binary_path)}
                    >
                      <Zap size={13} className="text-accent-soft shrink-0" />
                      <span className="min-w-0">
                        <span className="text-xs font-mono text-ink truncate block">{b.label}</span>
                        <span className="text-[0.625rem] font-mono text-faint truncate block">
                          {b.binary_path}
                        </span>
                      </span>
                    </button>
                  ))}
                </div>
              </div>
            )}

            {rtProgress && (
              <div className="card">
                <div className="flex items-center justify-between mb-2">
                  <span className="text-sm text-ink">Downloading runtime…</span>
                  <span className="text-xs font-mono text-dim">{progressText(rtProgress)}</span>
                </div>
                <Bar progress={rtProgress} />
                <div className="flex justify-end gap-1 mt-2">
                  <button
                    className="btn-ghost text-xs"
                    onClick={() => haltRuntime("Download paused — press Download to resume.")}
                  >
                    Pause
                  </button>
                  <button
                    className="btn-ghost text-xs"
                    onClick={() => haltRuntime("Download cancelled — press Download to resume.")}
                  >
                    Cancel
                  </button>
                </div>
              </div>
            )}

            {!runtimeReady && !rtProgress && (
              <>
                {!checked ? (
                  <div className="flex items-center gap-3">
                    <button className="btn-primary text-sm" onClick={probeBuilds} disabled={checking}>
                      {checking ? (
                        <>
                          <RefreshCw size={14} className="animate-spin" /> Checking…
                        </>
                      ) : (
                        <>
                          <Download size={14} /> Check for available builds
                        </>
                      )}
                    </button>
                    <button className="btn-ghost text-sm" onClick={locateRuntime}>
                      <FolderOpen size={14} /> Use existing installation
                    </button>
                  </div>
                ) : (
                  <>
                    <p className="text-xs text-dim">
                      Checked {release?.tag_name ?? "latest release"}
                      {builds.length === 0 ? " — no builds matched this system." : "."}
                    </p>
                    {builds.length > 0 && (
                      <div className="space-y-1.5">
                        {builds.map((a) => (
                          <AssetOption
                            key={a.name}
                            asset={a}
                            selected={asset === a.name}
                            onSelect={() => setAsset(a.name)}
                          />
                        ))}
                      </div>
                    )}
                    <div className="flex items-center gap-3">
                      {asset && builds.length > 0 && (
                        <button className="btn-primary text-sm" onClick={installRuntime}>
                          <Download size={14} /> Download
                        </button>
                      )}
                      <button className="btn-ghost text-sm" onClick={probeBuilds} disabled={checking}>
                        <RefreshCw size={14} className={checking ? "animate-spin" : ""} />
                        Refresh
                      </button>
                      <button className="btn-ghost text-sm" onClick={locateRuntime}>
                        <FolderOpen size={14} /> Use existing installation
                      </button>
                    </div>
                  </>
                )}
              </>
            )}
          </div>
        )}

        {step === 2 && (
          <div className="max-w-3xl mx-auto space-y-5">
            <div>
              <h2 className="section-title">Models</h2>
              <p className="section-desc">
                werk. scans your model folders; the list is sorted by fit for your{" "}
                {vram > 0 ? `${mbToGb(vram)} VRAM + ` : ""}
                {mbToGb(ram)} RAM — pick up to 3 to download.
              </p>
            </div>

            {Object.keys(modelErrors).length > 0 && (
              <div className="card border-accent-red/30 bg-accent-red/5">
                {Object.entries(modelErrors).map(([f, err]) => (
                  <p key={f} className="text-xs text-accent-red">
                    {f}: {err}
                  </p>
                ))}
              </div>
            )}

            <div className="card flex items-center gap-3">
              <FolderOpen size={14} className="text-accent-soft shrink-0" />
              <div className="flex-1 min-w-0">
                <p className="text-xs text-dim">Models folder</p>
                <p className="text-xs font-mono text-ink truncate" title={modelsDir ?? ""}>
                  {modelsDir || "Not set"}
                </p>
                {dirError && <p className="text-xs text-accent-red mt-0.5">{dirError}</p>}
              </div>
              <button className="btn-secondary text-xs shrink-0" onClick={changeModelsDir}>
                Change…
              </button>
            </div>

            <div className="space-y-1.5">
              {ordered.map((m) => {
                const fit = FIT_BADGE[fitFor(m.estimated_size_mb, vram, ram)];
                const picked = picks.has(m.filename);
                const ready = m.installed || installed.has(m.filename);
                const progress = dlProgress[m.filename];
                const path = locations[m.filename];
                const error = modelErrors[m.filename];
                return (
                  <div
                    key={m.filename}
                    className={`px-3 py-3 border rounded transition-colors ${
                      ready
                        ? "border-accent-green/30 bg-accent-green/5"
                        : picked
                          ? "border-accent/60 bg-accent/10"
                          : "border-border hover:bg-surface-2"
                    }`}
                  >
                    <button
                      className="w-full text-left flex items-center gap-3 disabled:cursor-default"
                      disabled={ready || queue}
                      onClick={() => togglePick(m.filename)}
                    >
                      <span
                        className={`w-3.5 h-3.5 border-2 shrink-0 flex items-center justify-center ${
                          ready
                            ? "border-accent-green bg-accent-green"
                            : picked
                              ? "border-accent bg-accent"
                              : "border-faint"
                        }`}
                      >
                        {(ready || picked) && <CircleCheck size={10} className="text-white" />}
                      </span>
                      <span className="flex-1 min-w-0 block">
                        <span className="flex items-center gap-2 flex-wrap">
                          <span className="text-sm font-medium text-ink">{m.name}</span>
                          <span className={`text-[0.625rem] ${fit.cls}`}>{fit.text}</span>
                          {ready && <span className="badge-green text-[0.625rem]">Installed</span>}
                        </span>
                        <span className="block text-xs text-dim mt-0.5">{m.description}</span>
                        <span className="flex gap-3 mt-0.5 text-xs text-dim">
                          <span>{m.params_b}B params</span>
                          <span>{m.quant}</span>
                          <span>~{mbToGb(m.estimated_size_mb)}</span>
                        </span>
                        {path && (
                          <span
                            className="block text-[0.625rem] font-mono text-faint truncate mt-0.5"
                            title={path}
                          >
                            {path}
                          </span>
                        )}
                      </span>
                    </button>
                    {progress && (
                      <div className="mt-2 flex items-center gap-2">
                        <div className="flex-1 min-w-0">
                          <Bar progress={progress} />
                          <p className="text-xs text-dim mt-0.5">{progressText(progress)}</p>
                        </div>
                        <button
                          className="btn-ghost text-xs shrink-0"
                          onClick={() => haltModel(m.filename, "Paused — press Download to resume.")}
                        >
                          Pause
                        </button>
                        <button
                          className="btn-ghost text-xs shrink-0"
                          onClick={() =>
                            haltModel(m.filename, "Cancelled — press Download to resume.")
                          }
                        >
                          Cancel
                        </button>
                      </div>
                    )}
                    {error && <p className="text-xs text-accent-red mt-1">{error}</p>}
                  </div>
                );
              })}
            </div>
          </div>
        )}

        {step === 3 && (
          <div className="max-w-3xl mx-auto space-y-5">
            <div>
              <h2 className="section-title">Appearance</h2>
              <p className="section-desc">
                Pick a look. System follows your OS light/dark setting. This is a live preview.
              </p>
            </div>

            <div className="grid grid-cols-2 sm:grid-cols-4 gap-3">
              {THEME_OPTIONS.map((opt) => {
                const active = theme === opt.value;
                return (
                  <button
                    key={opt.value}
                    onClick={() => setTheme(opt.value)}
                    className={`flex flex-col items-center gap-1.5 px-3 py-3 rounded border text-center transition-colors ${
                      active
                        ? "border-accent bg-accent/10 text-ink"
                        : "border-border bg-surface-1 hover:bg-surface-2 text-dim"
                    }`}
                  >
                    <ThemeIcon theme={opt.value} className={active ? "text-accent-soft" : "text-dim"} />
                    <span className="text-xs font-medium">{opt.label}</span>
                    <span className="text-[0.625rem] text-dim leading-tight">{opt.description}</span>
                  </button>
                );
              })}
            </div>

            <div>
              <p className="label mb-1.5">Accent</p>
              <div className="flex items-center gap-2">
                {(Object.keys(accentPalette()) as AccentName[]).map((a) => (
                  <button
                    key={a}
                    title={accentPalette()[a].label}
                    onClick={() => setAccent(a)}
                    className={`w-6 h-6 rounded-full border-2 transition-transform ${
                      accent === a ? "border-ink scale-110" : "border-transparent"
                    }`}
                    style={{ backgroundColor: `rgb(${accentPalette()[a].rgb})` }}
                  />
                ))}
              </div>
            </div>

            <div>
              <p className="label mb-1.5">
                Corners <span className="text-faint">— Auto follows your OS default.</span>
              </p>
              <div className="flex items-center gap-2">
                {(["auto", "sharp", "round"] as CornersMode[]).map((c) => (
                  <button
                    key={c}
                    onClick={() => setCorners(c)}
                    className={`px-2.5 py-1.5 rounded border text-xs font-medium capitalize transition-colors ${
                      corners === c
                        ? "border-accent bg-accent/20 text-ink"
                        : "border-border bg-surface-1 text-dim hover:text-ink"
                    }`}
                  >
                    {c}
                  </button>
                ))}
              </div>
            </div>
          </div>
        )}
      </div>

      <div className="flex items-center gap-3 h-14 px-4 shrink-0 border-t border-border">
        <div className="flex flex-1 items-center gap-3 min-w-0">
          {step > 1 && (
            <button
              className="btn-ghost text-sm shrink-0"
              onClick={() => setStep((step - 1) as Step)}
            >
              <ChevronLeft size={14} /> Back
            </button>
          )}
          <span className="text-xs text-dim truncate">{stepHint}</span>
        </div>
        <div className="flex items-center gap-2 shrink-0">
          <button className="btn-ghost text-sm" onClick={finish}>
            Skip wizard
          </button>
          {step === 1 && (
            <button className="btn-primary text-sm" onClick={() => setStep(2)}>
              {runtimeReady ? "Next" : "Skip this step"} <ChevronRight size={14} />
            </button>
          )}
          {step === 2 &&
            (queue ? (
              <button className="btn-primary text-sm" disabled>
                <RefreshCw size={14} className="animate-spin" /> Downloading…
              </button>
            ) : picks.size > 0 && !picksDone ? (
              <button className="btn-primary text-sm" onClick={downloadPicks}>
                <Download size={14} /> Download {picks.size} model
                {picks.size === 1 ? "" : "s"}
              </button>
            ) : (
              <button className="btn-primary text-sm" onClick={() => setStep(3)}>
                Next <ChevronRight size={14} />
              </button>
            ))}
          {step === 3 && (
            <button className="btn-primary text-sm" onClick={finish}>
              Finish
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

function StatTile({
  icon: Icon,
  label,
  value,
  detail,
}: {
  icon: typeof Cpu;
  label: string;
  value: string;
  detail?: string;
}) {
  return (
    <div className="rounded border border-border bg-surface-2 p-2.5 min-w-0">
      <div className="flex items-center gap-1.5 text-faint">
        <Icon size={12} className="shrink-0" />
        <span className="text-[0.625rem] uppercase tracking-wider">{label}</span>
      </div>
      <p className="mt-1 text-xs font-medium text-ink truncate" title={value}>
        {value}
      </p>
      {detail && (
        <p className="mt-0.5 text-[0.625rem] text-faint truncate" title={detail}>
          {detail}
        </p>
      )}
    </div>
  );
}

function AssetOption({
  asset,
  selected,
  onSelect,
}: {
  asset: AssetDto;
  selected: boolean;
  onSelect: () => void;
}) {
  return (
    <button
      className={`w-full flex items-center gap-3 px-3 py-2.5 border rounded text-left transition-colors ${
        selected ? "border-accent/60 bg-accent/10" : "border-border hover:bg-surface-2"
      }`}
      onClick={onSelect}
    >
      <span
        className={`w-3 h-3 rounded-full border-2 shrink-0 ${
          selected ? "border-accent bg-accent" : "border-faint"
        }`}
      />
      <span className="flex-1 min-w-0 block">
        <span className="flex items-center gap-2">
          <span className="text-xs font-medium text-ink truncate">{asset.name}</span>
          {asset.score >= 90 && (
            <span className="badge-green text-[0.625rem] shrink-0">Recommended</span>
          )}
        </span>
        <span className="flex gap-3 mt-0.5">
          <span className="text-xs text-dim">{asset.backend_label}</span>
          <span className="text-xs text-faint">{mbToGb(asset.size_mb ?? 0)}</span>
        </span>
      </span>
    </button>
  );
}
