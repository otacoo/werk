import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import type { Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { openUrl } from "@tauri-apps/plugin-opener";
import {
  ArrowUpCircle,
  Bug,
  Download,
  ExternalLink,
  Info,
  Palette,
  Play,
  RefreshCw,
  SlidersHorizontal,
  type LucideIcon,
} from "lucide-react";
import { commands } from "../bindings";
import type { AppConfig } from "../bindings";
import { call } from "../utils/ipc";
import { checkForUpdate, subscribeUpdate } from "../utils/updates";
import { useAppConfig } from "../utils/useAppConfig";
import Toggle from "./Toggle";
import { THEME_OPTIONS, ThemeIcon } from "./ThemeIcon";
import { notifyConfigChanged } from "../utils/appSettings";
import { playNotificationSound, testNotificationSound } from "../utils/sounds";
import { getDevtools, setDevtools } from "../utils/debugPrefs";
import {
  FONT_SIZES,
  accentPalette,
  getAccent,
  getCorners,
  getFont,
  getShowToolSnippets,
  getShowRunChanges,
  getTheme,
  getBubbleAlign,
  setAccent,
  setCorners,
  setFont,
  setShowToolSnippets,
  setShowRunChanges,
  setTheme,
  setBubbleAlign,
  subscribeAppearance,
  subscribeShowToolSnippets,
  subscribeShowRunChanges,
  subscribeBubbleAlign,
  type AccentName,
  type CornersMode,
  type FontSizeName,
  type ThemeName,
} from "../utils/appearance";

const REPO_URL = "https://github.com/otacoo/werk";

export type SettingsSection = "general" | "appearance" | "debug" | "about";

function GeneralCard({ appConfig, setAppConfig, refresh }: {
  appConfig: AppConfig | null;
  setAppConfig: React.Dispatch<React.SetStateAction<AppConfig | null>>;
  refresh: () => Promise<void>;
}) {
  const [error, setError] = useState<string | null>(null);
  const [showSnippets, setShowSnippets] = useState(getShowToolSnippets());
  useEffect(() => subscribeShowToolSnippets(setShowSnippets), []);
  const [bubbleAlign, setBubbleAlignState] = useState(getBubbleAlign());
  useEffect(() => subscribeBubbleAlign(setBubbleAlignState), []);
  const [showChanges, setShowChangesState] = useState(getShowRunChanges());
  useEffect(() => subscribeShowRunChanges(setShowChangesState), []);
  const confirm = async (apply: (c: AppConfig) => AppConfig, save: () => Promise<unknown>) => {
    setAppConfig((c) => (c ? apply(c) : c));
    try {
      await save();
      setError(null);
      await refresh();
    } catch (e) {
      setError(String(e));
      await refresh();
    }
    // Pages holding config snapshots (Run, Chat) refetch instead of going stale.
    notifyConfigChanged();
  };
  const setSound = async (key: "agent" | "permissions" | "errors", v: boolean) => {
    const cmd =
      key === "agent"
        ? commands.setSoundAgent(v)
        : key === "permissions"
          ? commands.setSoundPermissions(v)
          : commands.setSoundErrors(v);
    const apply = (c: AppConfig) =>
      key === "agent"
        ? { ...c, sound_agent: v }
        : key === "permissions"
          ? { ...c, sound_permissions: v }
          : { ...c, sound_errors: v };
    await confirm(apply, () => call(cmd));
    if (v) void playNotificationSound(key);
  };

  const rows: { key: "agent" | "permissions" | "errors"; label: string; hint: string }[] = [
    { key: "agent", label: "Agent", hint: "Play a sound when the agent finishes a prompt." },
    { key: "permissions", label: "Permissions", hint: "Play a sound when the agent needs your attention." },
    { key: "errors", label: "Errors", hint: "Play a sound when an error occurs." },
  ];
  const checked = (key: "agent" | "permissions" | "errors") =>
    key === "agent"
      ? (appConfig?.sound_agent ?? true)
      : key === "permissions"
        ? (appConfig?.sound_permissions ?? true)
        : (appConfig?.sound_errors ?? true);

  return (
    <>
      <div className="card">
        <h2 className="section-title mb-1">General</h2>
        <p className="section-desc">Core app behavior.</p>
        <div className="space-y-3 mt-3">
          <Toggle
            label="Show in notification area"
            hint="Closing hides werk. to the tray instead of quitting."
            checked={appConfig?.close_to_tray ?? false}
            onChange={(v) =>
              confirm((c) => ({ ...c, close_to_tray: v }), () => call(commands.setCloseToTray(v)))
            }
          />
          <Toggle
            label="Enable Quick Bench"
            hint="Toggle benchmarking tools."
            checked={appConfig?.bench_visible ?? true}
            onChange={(v) =>
              confirm((c) => ({ ...c, bench_visible: v }), () => call(commands.setBenchVisible(v)))
            }
          />
          {error && <p className="text-xs text-accent-red">{error}</p>}
        </div>
      </div>
      <div className="card">
        <h2 className="section-title mb-1">Chat</h2>
        <p className="section-desc">Chat transcript display options.</p>
        <div className="space-y-3 mt-3">
          <Toggle
            label="Show tool call snippets"
            hint="Off keeps collapsed tool cards to name and status; arguments show on expand."
            checked={showSnippets}
            onChange={setShowToolSnippets}
          />
          <Toggle
            label="Show changed files summary"
            hint="After a run, list the git diff for the files the agent changed (max. 5 shown)."
            checked={showChanges}
            onChange={setShowRunChanges}
          />
          <div>
            <p className="label mb-1.5 font-bold text-ink">Chat bubble alignment:</p>
            <div className="flex gap-0.5">
              {(["left", "right"] as const).map((v) => (
                <button
                  key={v}
                  onClick={() => setBubbleAlign(v)}
                  className={`px-2.5 py-1 text-xs font-medium rounded transition-colors ${
                    bubbleAlign === v
                      ? "bg-accent/20 text-ink"
                      : "text-dim hover:text-ink hover:bg-accent/10"
                  }`}
                >
                  {v === "left" ? "Left" : "Right"}
                </button>
              ))}
            </div>
            <p className="section-desc mt-1.5">
              User messages on the {bubbleAlign} side; the model takes the other side.
            </p>
          </div>
        </div>
      </div>
      <div className="card">
        <h2 className="section-title mb-1">Notifications</h2>
        <p className="section-desc">Sound effects for agent activity.</p>
        <div className="mt-3 divide-y divide-border">
          {rows.map(({ key, label, hint }) => (
            <div key={key} className="py-2.5 first:pt-0 last:pb-0">
              <Toggle label={label} hint={hint} checked={checked(key)} onChange={(v) => setSound(key, v)} />
            </div>
          ))}
        </div>
      </div>
      <UpdatesCard />
    </>
  );
}

function UpdatesCard() {
  const [appVersion, setAppVersion] = useState<string | null>(null);
  const [autoCheck, setAutoCheck] = useState(false);
  const [updateAvailable, setUpdateAvailable] = useState(false);
  const [updateVersion, setUpdateVersion] = useState<string | null>(null);
  const [pendingUpdate, setPendingUpdate] = useState<Update | null>(null);
  const [checking, setChecking] = useState(false);
  const [checked, setChecked] = useState(false);
  const [updating, setUpdating] = useState(false);
  const [downloadPercent, setDownloadPercent] = useState<number | null>(null);
  const [installNote, setInstallNote] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    getVersion().then(setAppVersion).catch(() => {});
    // The startup check (App) publishes here too, so the card shows the
    // result without checking twice.
    const unsubscribe = subscribeUpdate((update, hasChecked) => {
      setUpdateAvailable(update != null);
      setUpdateVersion(update?.version ?? null);
      setPendingUpdate(update);
      if (hasChecked) setChecked(true);
    });
    call(commands.getConfig())
      .then((c) => {
        const on = c.auto_check_updates ?? false;
        setAutoCheck(on);
        if (on) void runCheck();
      })
      .catch(() => {});
    return unsubscribe;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const runCheck = async () => {
    setChecking(true);
    setChecked(false);
    setError(null);
    try {
      await checkForUpdate();
    } catch (e) {
      setError(String(e));
    } finally {
      setChecking(false);
      setChecked(true);
    }
  };

  const installUpdate = async () => {
    if (!pendingUpdate) return;
    setUpdating(true);
    setDownloadPercent(null);
    setInstallNote(null);
    try {
      let total = 0;
      let downloaded = 0;
      await pendingUpdate.downloadAndInstall((event) => {
        switch (event.event) {
          case "Started":
            total = event.data.contentLength ?? 0;
            downloaded = 0;
            break;
          case "Progress":
            downloaded += event.data.chunkLength;
            if (total > 0) {
              setDownloadPercent(Math.min(100, (downloaded / total) * 100));
            }
            break;
          case "Finished":
            setDownloadPercent(100);
            break;
        }
      });
      setInstallNote("Installer launched — werk. will close and reopen.");
      await relaunch();
    } catch (e) {
      setError(String(e));
      setUpdating(false);
    }
  };

  return (
    <div className="card">
      <div className="flex items-center justify-between mb-3">
        <h2 className="section-title mb-0">App Updates</h2>
        {appVersion && (
          <span className="text-xs text-dim tabular-nums">v{appVersion}</span>
        )}
      </div>
      <div className="space-y-3">
        <Toggle
          label="Check for updates on app start"
          checked={autoCheck}
          onChange={async (v) => {
            setAutoCheck(v);
            try {
              await call(commands.setAutoCheckUpdates(v));
            } catch {}
            if (v) void runCheck();
          }}
        />
        <div className="flex flex-wrap items-center gap-3">
          <button
            className="btn-secondary text-xs"
            onClick={runCheck}
            disabled={checking || updating}
          >
            <RefreshCw size={13} className={checking ? "animate-spin" : ""} />
            Check now
          </button>
          {checking && <span className="text-xs text-dim">Checking…</span>}
          {error && <span className="text-xs text-accent-red">{error}</span>}
          {checked && !error && updateAvailable && !updating && (
            <button className="btn-primary text-xs" onClick={installUpdate}>
              <Download size={13} />
              Update to v{updateVersion}
            </button>
          )}
          {checked && !error && updateAvailable && updating && (
            <span className="flex items-center gap-2 text-xs text-dim">
              <ArrowUpCircle size={13} className="text-accent-soft" />
              {downloadPercent != null
                ? `Downloading update… ${downloadPercent.toFixed(0)}%`
                : "Downloading update…"}
            </span>
          )}
          {checked && !error && !updateAvailable && !updating && (
            <span className="text-xs text-accent-green">Up to date</span>
          )}
        </div>
        {updating && downloadPercent != null && (
          <div className="w-full h-1.5 bg-surface-3 rounded overflow-hidden">
            <div
              className="h-full bg-accent transition-all duration-150"
              style={{ width: `${downloadPercent}%` }}
            />
          </div>
        )}
        {installNote && <p className="text-xs text-accent-yellow">{installNote}</p>}
      </div>
    </div>
  );
}


/// Webview devtools: reopen on start while enabled, so users can inspect
/// errors without a dev build.
function DevtoolsCard() {
  const [on, setOn] = useState(getDevtools);
  const toggle = (v: boolean) => {
    setOn(v);
    setDevtools(v);
    void call(commands.setDevtools(v)).catch(() => {});
  };
  return (
    <div className="card">
      <h2 className="section-title mb-1">Developer console</h2>
      <p className="section-desc">
        Open the webview devtools to inspect errors and network activity. It reopens on
        start while enabled.
      </p>
      <div className="mt-3">
        <Toggle label="Open devtools" checked={on} onChange={toggle} />
      </div>
    </div>
  );
}

/// Play each notification sound and surface playback errors; some platform
/// audio stacks fail silently otherwise.
function SoundTestCard() {
  const [error, setError] = useState<string | null>(null);
  const rows: { key: "agent" | "permissions" | "errors"; label: string }[] = [
    { key: "agent", label: "Agent" },
    { key: "permissions", label: "Permissions" },
    { key: "errors", label: "Errors" },
  ];
  return (
    <div className="card">
      <h2 className="section-title mb-1">Sound test</h2>
      <p className="section-desc">Play each notification sound; playback errors appear here.</p>
      <div className="flex items-center gap-2 mt-3">
        {rows.map((r) => (
          <button
            key={r.key}
            className="btn-secondary text-xs py-1 px-2"
            onClick={() => {
              setError(null);
              testNotificationSound(r.key).catch((e) => setError(`${r.label}: ${e}`));
            }}
          >
            <Play size={12} /> {r.label}
          </button>
        ))}
      </div>
      {error && <p className="text-xs text-accent-red mt-2 break-words">{error}</p>}
    </div>
  );
}

/// Enable/disable Mode profiles; disabled ones disappear everywhere.
function ProfilesCard({
  appConfig,
  setAppConfig,
  refresh,
}: {
  appConfig: AppConfig | null;
  setAppConfig: React.Dispatch<React.SetStateAction<AppConfig | null>>;
  refresh: () => void;
}) {
  const [error, setError] = useState<string | null>(null);
  const webui = appConfig?.webui_enabled ?? false;

  const setWebui = async (v: boolean) => {
    setError(null);
    setAppConfig((c) => (c ? { ...c, webui_enabled: v } : c));
    try {
      await call(commands.setWebuiEnabled(v));
      refresh();
      notifyConfigChanged();
    } catch (e) {
      setError(String(e));
      refresh();
    }
  };

  return (
    <div className="card space-y-3">
      <div>
        <h2 className="section-title mb-0">Profiles</h2>
        <p className="section-desc">
          Enable or disable Mode profiles. A disabled profile disappears from the Mode page, the
          header tabs, and the launch options.
        </p>
      </div>
      <Toggle
        label="WebUI"
        hint="llama-server's own chat UI as a profile and tab, plus the WebUI launch options."
        checked={webui}
        onChange={setWebui}
      />
      {error && <p className="text-xs text-accent-red">{error}</p>}
    </div>
  );
}

function AppearanceCard() {
  const [theme, setThemeState] = useState<ThemeName>("system");
  const [accent, setAccentState] = useState<AccentName>("blue");
  const [font, setFontState] = useState<FontSizeName>("default");
  const [corners, setCornersState] = useState<CornersMode>("auto");

  useEffect(() => {
    setThemeState(getTheme());
    setAccentState(getAccent());
    setFontState(getFont());
    setCornersState(getCorners());
    const unsubAppearance = subscribeAppearance(() => {
      setThemeState(getTheme());
      setAccentState(getAccent());
      setFontState(getFont());
      setCornersState(getCorners());
    });
    return unsubAppearance;
  }, []);

  const pick = <T extends string>(values: readonly T[], current: T, onPick: (v: T) => void) => (
    <div className="flex flex-wrap gap-1.5">
      {values.map((x) => (
        <button
          key={x}
          onClick={() => onPick(x)}
          className={`px-2.5 py-1 text-xs rounded transition-colors ${
            current === x ? "bg-accent/20 text-ink" : "text-dim hover:text-ink hover:bg-accent/10"
          }`}
        >
          {x}
        </button>
      ))}
    </div>
  );

  return (
    <>
      <div className="card">
        <h2 className="section-title mb-1">Theme</h2>
        <p className="section-desc">Choose how Werk looks.</p>
        <div className="grid grid-cols-2 gap-2 mt-3">
          {THEME_OPTIONS.map((opt) => {
            const active = theme === opt.value;
            return (
              <button
                key={opt.value}
                onClick={() => {
                  setTheme(opt.value);
                  setThemeState(opt.value);
                }}
                className={`flex flex-col items-center gap-1.5 px-3 py-3 rounded border text-center transition-colors ${
                  active
                    ? "border-accent bg-accent/10 text-ink"
                    : "border-border bg-surface-2 hover:bg-surface-3 text-dim"
                }`}
              >
                <ThemeIcon theme={opt.value} className={active ? "text-accent" : "text-faint"} />
                <span className="text-xs font-medium">{opt.label}</span>
                <span className="text-[0.625rem] text-faint leading-tight">{opt.description}</span>
              </button>
            );
          })}
        </div>
      </div>
      <div className="card">
        <div className="grid grid-cols-2 divide-x divide-border">
          <div className="pr-4">
            <h2 className="section-title mb-1">Accent</h2>
            <p className="section-desc">Highlight color used across the app.</p>
            <div className="flex flex-wrap gap-1.5 mt-2">
              {(Object.keys(accentPalette()) as AccentName[]).map((a) => (
                <button
                  key={a}
                  title={accentPalette()[a].label}
                  onClick={() => {
                    setAccent(a);
                    setAccentState(a);
                  }}
                  className={`w-7 h-7 rounded-full border-2 transition-transform ${
                    accent === a ? "border-ink scale-110" : "border-transparent"
                  }`}
                  style={{ backgroundColor: `rgb(${accentPalette()[a].rgb})` }}
                />
              ))}
            </div>
          </div>
          <div className="pl-4">
            <h2 className="section-title mb-1">Corners</h2>
            <p className="section-desc">Auto follows OS default.</p>
            <div className="mt-2">
              {pick(["auto", "sharp", "round"] as const, corners, (v) => {
                setCorners(v);
                setCornersState(v);
              })}
            </div>
          </div>
        </div>
      </div>
      <div className="card">
        <h2 className="section-title mb-1">Font</h2>
        <p className="section-desc">Global text size for the whole app.</p>
        <div className="flex flex-wrap gap-1.5 mt-2">
          {(Object.keys(FONT_SIZES) as FontSizeName[]).map((f) => (
            <button
              key={f}
              onClick={() => {
                setFont(f);
                setFontState(f);
              }}
              className={`px-2.5 py-1 text-xs rounded transition-colors ${
                font === f ? "bg-accent/20 text-ink" : "text-dim hover:text-ink hover:bg-accent/10"
              }`}
            >
              {FONT_SIZES[f].label}
            </button>
          ))}
        </div>
      </div>
    </>
  );
}

export default function SettingsPanel({ open, onClose, section, onSectionChange, onRestartWizard }: {
  open: boolean;
  onClose: () => void;
  section: SettingsSection;
  onSectionChange: (section: SettingsSection) => void;
  onRestartWizard: () => void;
}) {
  const [appConfig, setAppConfig, configError, refreshConfig] = useAppConfig(open);

  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open, onClose]);

  if (!open) return null;
  const visible: { id: SettingsSection; label: string; icon: LucideIcon }[] = [
    { id: "general", label: "General", icon: SlidersHorizontal },
    { id: "appearance", label: "Appearance", icon: Palette },
    { id: "debug", label: "Debug", icon: Bug },
    { id: "about", label: "About", icon: Info },
  ];

  return (
    <div className="absolute inset-0 z-40 flex bg-surface-0">
      <aside className="w-44 shrink-0 border-r border-border bg-surface-1 p-3 overflow-y-auto">
        <div className="flex items-center justify-between px-2 pb-3 pt-1">
          <span className="text-sm font-semibold text-ink">Settings</span>
          <button className="text-faint hover:text-ink transition-colors" onClick={onClose} title="Close (Esc)">
            ×
          </button>
        </div>
        <div className="space-y-0.5">
          {visible.map(({ id, label, icon: Icon }) => (
            <button
              key={id}
              onClick={() => onSectionChange(id)}
              className={`w-full flex items-center gap-2 px-2.5 py-1.5 rounded text-xs font-medium transition-colors ${
                section === id ? "bg-accent/20 text-ink" : "text-dim hover:text-ink hover:bg-accent/10"
              }`}
            >
              <Icon size={13} />
              {label}
            </button>
          ))}
        </div>
      </aside>
      <div className="flex-1 overflow-y-auto p-6">
        <div className="max-w-3xl space-y-4">
          {configError && (
            <div className="card border-accent-red/30 bg-accent-red/5">
              <p className="text-sm text-accent-red">{configError}</p>
            </div>
          )}
          {section === "general" && (
            <GeneralCard appConfig={appConfig} setAppConfig={setAppConfig} refresh={refreshConfig} />
          )}
          {section === "appearance" && <AppearanceCard />}
          {section === "debug" && (
            <>
              <div className="card">
                <h2 className="section-title mb-1">Setup wizard</h2>
                <p className="section-desc">
                  Run the first-launch setup again: hardware overview, runtime install, model
                  downloads, and appearance.
                </p>
                <button className="btn-secondary text-xs mt-3" onClick={onRestartWizard}>
                  <RefreshCw size={12} /> Start wizard again
                </button>
              </div>
              <ProfilesCard appConfig={appConfig} setAppConfig={setAppConfig} refresh={refreshConfig} />
              <DevtoolsCard />
              <SoundTestCard />
            </>
          )}
          {section === "about" && (
            <div className="card text-center">
              <p className="text-3xl font-bold tracking-tight select-none">
                werk<span className="text-accent">.</span>
              </p>
              <p className="text-sm text-dim mt-3">
                A llama.cpp launcher and agent harness, licensed under the{" "}
                <button
                  className="text-accent-soft hover:underline"
                  onClick={() => void openUrl(`${REPO_URL}/blob/main/LICENSE`).catch(() => {})}
                >
                  Apache License 2.0
                </button>
              </p>
              <p className="text-xs text-dim mt-4 leading-snug">
                Werk bundles open-source components — among them{" "}
                <button
                  className="text-accent-soft hover:underline"
                  onClick={() => void openUrl("https://katex.org").catch(() => {})}
                >
                  KaTeX
                </button>{" "}
                (MIT) for math rendering. The full list, with license texts, is in the{" "}
                <button
                  className="text-accent-soft hover:underline"
                  onClick={() =>
                    void openUrl(`${REPO_URL}/blob/main/THIRD-PARTY-NOTICES.md`).catch(() => {})
                  }
                >
                  third-party notices
                </button>
                , shipped alongside the app.
              </p>
              <button
                className="inline-flex items-center gap-1.5 text-xs text-dim hover:text-ink mt-4"
                onClick={() => void openUrl(REPO_URL).catch(() => {})}
              >
                <ExternalLink size={12} /> github.com/otacoo/werk
              </button>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
