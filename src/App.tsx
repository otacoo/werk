import { useEffect, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getVersion } from "@tauri-apps/api/app";
import {
  Brain,
  FlaskConical,
  LayoutDashboard,
  MessageSquare,
  Network,
  Play,
  Plug,
  Settings as SettingsIcon,
  Sparkles,
  Wrench,
  type LucideIcon,
} from "lucide-react";
import Dashboard from "./pages/Dashboard";
import Run from "./pages/Run";
import Chat from "./pages/Chat";
import Bench from "./pages/Bench";
import Tools from "./pages/Tools";
import Agent from "./pages/Agent";
import Roleplay from "./pages/Roleplay";
import Mode from "./pages/Mode";
import Api from "./pages/Api";
import Wizard from "./pages/Wizard";
import WindowControls from "./components/WindowControls";
import SettingsPanel, { type SettingsSection } from "./components/SettingsPanel";
import { commands } from "./bindings";
import { call } from "./utils/ipc";
import { loadAppearance, setAutoCorners } from "./utils/appearance";
import { getQuickBench, setQuickBench, subscribeQuickBench, subscribeConfigChanged } from "./utils/appSettings";
import { startServerStatusPolling, subscribeServerStatus } from "./utils/serverStatus";

export type Tab = "dashboard" | "run" | "chat" | "tools" | "agent" | "roleplay" | "bench" | "mode" | "api";

const TABS: { id: Tab; label: string; icon: LucideIcon }[] = [
  { id: "dashboard", label: "Dashboard", icon: LayoutDashboard },
  { id: "mode", label: "Mode", icon: Network },
  { id: "tools", label: "Tools", icon: Wrench },
  { id: "agent", label: "Agent", icon: Brain },
  { id: "roleplay", label: "Roleplay", icon: Sparkles },
  { id: "api", label: "API", icon: Plug },
  { id: "bench", label: "Bench", icon: FlaskConical },
  { id: "run", label: "Run", icon: Play },
  { id: "chat", label: "Chat", icon: MessageSquare },
];

export default function App() {
  const [tab, setTab] = useState<Tab>("dashboard");
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [settingsSection, setSettingsSection] = useState<SettingsSection>("general");
  const [wizard, setWizard] = useState<boolean | null>(null);
  const [showBench, setShowBench] = useState(() => getQuickBench());
  const [appVersion, setAppVersion] = useState<string | null>(null);
  const [serverRunning, setServerRunning] = useState(false);
  /// External API mode: the local server (and therefore Run) is not used.
  const [externalMode, setExternalMode] = useState(false);

  const closeSettings = () => setSettingsOpen(false);

  // Run and Chat light up while the server is up.
  useEffect(() => {
    startServerStatusPolling();
    return subscribeServerStatus((s) =>
      setServerRunning(s.type === "running" || s.type === "starting"),
    );
  }, []);

  useEffect(() => {
    loadAppearance();
    call(commands.getConfig())
      .then((c) => {
        setWizard(!c.wizard_completed);
        setQuickBench(c.bench_visible ?? false);
        setExternalMode(c.server_mode === "external");
      })
      .catch(() => setWizard(false));
    call(commands.getPlatformStyle())
      .then((p) => {
        if (p.corners === "round" || p.corners === "sharp") setAutoCorners(p.corners);
      })
      .catch(() => {});
    getVersion().then(setAppVersion).catch(() => {});
  }, []);

  // The bench tab and Run card follow the Settings toggle live; fall back to
  // Dashboard when the open tab gets hidden.
  useEffect(
    () =>
      subscribeQuickBench((visible) => {
        setShowBench(visible);
        if (!visible) setTab((t) => (t === "bench" ? "dashboard" : t));
      }),
    [],
  );

  // Mode switches come from the Mode page; Run and API lock while External
  // API is on (no local server to talk to).
  useEffect(
    () =>
      subscribeConfigChanged(() => {
        call(commands.getConfig())
          .then((c) => {
            const external = c.server_mode === "external";
            setExternalMode(external);
            if (external) setTab((t) => (t === "run" || t === "api" ? "chat" : t));
          })
          .catch(() => {});
      }),
    [],
  );

  // The window starts hidden; reveal it as soon as the first screen is ready
  // (or after a fallback delay) so startup never shows a blank frame.
  const revealed = useRef(false);
  useEffect(() => {
    const reveal = () => {
      if (revealed.current) return;
      revealed.current = true;
      getCurrentWindow().show().catch(() => {});
    };
    if (wizard !== null) reveal();
    const fallback = setTimeout(reveal, 2000);
    return () => clearTimeout(fallback);
  }, [wizard]);

  if (wizard === null) return null;
  if (wizard) return <Wizard onDone={() => setWizard(false)} />;

  return (
    <div className="h-screen overflow-hidden bg-surface-0 text-ink flex flex-col border border-border">
      <div className="relative flex items-center h-9 shrink-0 border-b border-border select-none">
        <div
          data-tauri-drag-region
          className="absolute inset-0"
          onDoubleClick={() => {
            getCurrentWindow()
              .toggleMaximize()
              .catch((e) => console.error("Maximize failed:", e));
          }}
        />
        <div className="relative z-10 flex-1 flex items-center px-3 min-w-0 pointer-events-none">
          <div className="flex items-center gap-2 min-w-0 pointer-events-auto">
            <div className="flex items-baseline gap-1.5 shrink-0">
              <p className="text-sm font-semibold">
                werk<span className="text-accent text-[1.1em]">.</span>
              </p>
              {appVersion && (
                <span className="text-[0.6875rem] font-mono text-faint">v{appVersion}</span>
              )}
              {appVersion && (
                <span aria-hidden className="text-xs text-faint/60 select-none">
                  |
                </span>
              )}
            </div>
            <nav className="flex items-center gap-0.5 min-w-0 overflow-x-auto">
              {TABS.filter(
                (t) =>
                  (t.id !== "bench" || showBench) &&
                  (t.id !== "api" || !externalMode) &&
                  (t.id !== "run" || !externalMode),
              ).map(({ id, label, icon: Icon }) => {
                // Server state colors Run (green) and Chat (bright) even selected.
                const highlight =
                  serverRunning && id === "run"
                    ? "text-accent-green"
                    : serverRunning && id === "chat"
                      ? "text-ink"
                      : null;
                // Filled glyphs while the server is up.
                const filled = serverRunning && (id === "run" || id === "chat");
                return (
                  <button
                    key={id}
                    onClick={() => {
                      setTab(id);
                      closeSettings();
                    }}
                    className={`flex items-center gap-1.5 px-2.5 py-1 rounded text-xs font-medium transition-colors whitespace-nowrap ${
                      tab === id ? "bg-accent/20" : "hover:bg-accent/10"
                    } ${highlight ?? (tab === id ? "text-ink" : "text-dim hover:text-ink")}`}
                  >
                    <Icon size={13} fill={filled ? "currentColor" : "none"} />
                    {label}
                  </button>
                );
              })}
          </nav>
          </div>
        </div>
        <div className="relative z-10 flex items-center shrink-0 pointer-events-auto">
          <button
            onClick={() => (settingsOpen ? closeSettings() : setSettingsOpen(true))}
            title="Settings"
            className={`w-9 h-9 shrink-0 inline-flex items-center justify-center transition-colors ${
              settingsOpen ? "bg-accent/20 text-ink" : "text-dim hover:text-ink hover:bg-surface-3"
            }`}
          >
            <SettingsIcon size={14} />
          </button>
          <WindowControls />
        </div>
      </div>
      <main className="flex-1 min-w-0 min-h-0 relative">
        {/* Chat, Run, Dashboard, Tools and Mode stay mounted off-tab: stream,
            launch, download and form state survive switches. */}
        <div className="h-full min-h-0" style={{ display: tab === "chat" ? undefined : "none" }}>
          <Chat go={setTab} active={tab === "chat"} />
        </div>
        <div className="h-full min-h-0" style={{ display: tab === "run" ? undefined : "none" }}>
          <Run go={setTab} />
        </div>
        <div className="h-full min-h-0" style={{ display: tab === "dashboard" ? undefined : "none" }}>
          <Dashboard go={setTab} />
        </div>
        {tab === "bench" && showBench && <Bench />}
        <div className="h-full min-h-0" style={{ display: tab === "tools" ? undefined : "none" }}>
          <Tools active={tab === "tools"} />
        </div>
        <div className="h-full min-h-0" style={{ display: tab === "agent" ? undefined : "none" }}>
          <Agent active={tab === "agent"} />
        </div>
        <div className="h-full min-h-0" style={{ display: tab === "roleplay" ? undefined : "none" }}>
          <Roleplay go={(t) => setTab(t)} />
        </div>
        <div className="h-full min-h-0" style={{ display: tab === "mode" ? undefined : "none" }}>
          <Mode go={(t) => setTab(t)} />
        </div>
        {tab === "api" && <Api />}
        <SettingsPanel
          open={settingsOpen}
          onClose={closeSettings}
          section={settingsSection}
          onSectionChange={setSettingsSection}
        />
      </main>
    </div>
  );
}
