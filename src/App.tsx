import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { getVersion } from "@tauri-apps/api/app";
import {
  Brain,
  Drama,
  FlaskConical,
  Globe,
  LayoutDashboard,
  MessageCircleHeart,
  MessageSquare,
  Network,
  Play,
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
import Assistant from "./pages/Assistant";
import AssistantChat from "./pages/AssistantChat";
import Talk from "./pages/Talk";
import WebUI from "./pages/WebUI";
import Mode from "./pages/Mode";
import Wizard from "./pages/Wizard";
import WindowControls from "./components/WindowControls";
import SettingsPanel, { type SettingsSection } from "./components/SettingsPanel";
import { commands } from "./bindings";
import { call } from "./utils/ipc";
import { loadAppearance, setAutoCorners } from "./utils/appearance";
import { getDevtools, getThemeAnimations, subscribeThemeAnimations } from "./utils/debugPrefs";
import { getQuickBench, setQuickBench, subscribeQuickBench, subscribeConfigChanged, setProfileMirror, subscribeProfile } from "./utils/appSettings";
import { startServerStatusPolling, subscribeServerStatus } from "./utils/serverStatus";
import { checkForUpdate, subscribeUpdate } from "./utils/updates";

export type Tab = "dashboard" | "run" | "chat" | "talk" | "assistant" | "assistant-chat" | "webui" | "tools" | "agent" | "roleplay" | "bench" | "mode";

type Profile = "agent" | "webui" | "roleplay" | "assistant";

const TABS: { id: Tab; label: string; icon: LucideIcon }[] = [
  { id: "dashboard", label: "Dashboard", icon: LayoutDashboard },
  { id: "mode", label: "Mode", icon: Network },
  { id: "tools", label: "Tools", icon: Wrench },
  { id: "agent", label: "Agent", icon: Brain },
  { id: "roleplay", label: "Roleplay", icon: Drama },
  { id: "bench", label: "Bench", icon: FlaskConical },
  { id: "assistant", label: "Assistant", icon: Sparkles },
  { id: "run", label: "Run", icon: Play },
  { id: "chat", label: "Chat", icon: MessageSquare },
  { id: "assistant-chat", label: "Chat", icon: MessageSquare },
  { id: "talk", label: "Talk", icon: MessageCircleHeart },
  { id: "webui", label: "WebUI", icon: Globe },
];

/// A tab is only valid for the active profile; anything else falls back to the
/// profile's own chat surface (or Dashboard) so hidden pages never paint.
function visibleTab(t: Tab, profile: Profile, externalMode: boolean): Tab {
  if (profile === "agent") {
    return t === "talk" || t === "roleplay" || t === "assistant" || t === "assistant-chat"
      ? "chat"
      : t;
  }
  if (profile === "webui") {
    if (t === "webui") return externalMode ? "dashboard" : t;
    if (
      t === "chat" ||
      t === "agent" ||
      t === "talk" ||
      t === "roleplay" ||
      t === "assistant" ||
      t === "assistant-chat"
    ) {
      return externalMode ? "dashboard" : "webui";
    }
    return t;
  }
  if (profile === "assistant") {
    if (t === "chat" || t === "agent" || t === "talk" || t === "roleplay" || t === "tools") {
      return "assistant-chat";
    }
    return t;
  }
  return t === "chat" || t === "agent" || t === "tools" || t === "assistant" || t === "assistant-chat"
    ? "talk"
    : t;
}

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
  /// Chat profile: which surfaces the header shows.
  const [profile, setProfile] = useState<Profile>("agent");
  /// WebUI profile availability (Settings → Profiles).
  const [webuiEnabled, setWebuiEnabled] = useState(false);
  /// Assistant overlay visibility (Assistant → Behavior).
  const [overlayEnabled, setOverlayEnabled] = useState(true);
  /// An update is waiting; the gear shows a dot.
  const [updateReady, setUpdateReady] = useState(false);

  const closeSettings = () => setSettingsOpen(false);

  // Run and Chat light up while the server is up.
  useEffect(() => {
    startServerStatusPolling();
    return subscribeServerStatus((s) =>
      setServerRunning(s.type === "running" || s.type === "starting"),
    );
  }, []);

  // Auto-check for updates a moment after the first screen is up; the check
  // must never compete with startup (the gear shows a dot while one is found).
  useEffect(() => {
    const unsubscribe = subscribeUpdate((update) => setUpdateReady(update != null));
    const timer = window.setTimeout(() => {
      call(commands.getConfig())
        .then((c) => {
          if (c.auto_check_updates) void checkForUpdate().catch(() => {});
        })
        .catch(() => {});
    }, 4000);
    return () => {
      window.clearTimeout(timer);
      unsubscribe();
    };
  }, []);

  // The Mode page flips the profile through the in-memory mirror.
  useEffect(() => subscribeProfile((p) => setProfile(p as Profile)), []);

  // Debug preference: keep the devtools open across starts while enabled.
  useEffect(() => {
    if (getDevtools()) void call(commands.setDevtools(true)).catch(() => {});
  }, []);

  // Software-rendered WebKitGTK (the NVIDIA workaround) cannot afford the
  // theme card orbits; a flag lets the CSS fall back to a static border. The
  // Debug toggle overrides that.
  useEffect(() => {
    let software = false;
    const apply = () => {
      const root = document.documentElement;
      if (software && !getThemeAnimations()) {
        root.dataset.reducedMotion = "1";
      } else {
        delete root.dataset.reducedMotion;
      }
    };
    call(commands.softwareRendering())
      .then((v) => {
        software = v;
        apply();
      })
      .catch(() => {});
    return subscribeThemeAnimations(apply);
  }, []);

  useEffect(() => {
    loadAppearance();
    call(commands.getConfig())
      .then((c) => {
        setWizard(!c.wizard_completed);
        setQuickBench(c.bench_visible ?? false);
        setExternalMode(c.server_mode === "external");
        setProfileMirror((c.chat_profile ?? "agent") as Profile);
        setCloseToTray(c.close_to_tray ?? false);
        setWebuiEnabled(c.webui_enabled ?? false);
        setOverlayEnabled(c.assistant?.overlay_enabled ?? false);
      })
      .catch(() => setWizard(false));
    call(commands.getPlatformStyle())
      .then((p) => {
        if (p.corners === "round" || p.corners === "sharp") setAutoCorners(p.corners);
      })
      .catch(() => {});
    getVersion().then(setAppVersion).catch(() => {});
  }, []);

  // Assistant always-on: the overlay follows the profile, and the app hides
  // to the tray once the server is running (shares close_to_tray, so the tray
  // icon is always there to reopen from).
  const [closeToTray, setCloseToTray] = useState(false);
  const minimizedRef = useRef(false);
  useEffect(() => {
    call(commands.setOverlayVisible(profile === "assistant" && overlayEnabled)).catch(() => {});
  }, [profile, overlayEnabled]);
  useEffect(() => {
    if (profile !== "assistant" || !serverRunning || !closeToTray) return;
    if (minimizedRef.current) return;
    minimizedRef.current = true;
    getCurrentWindow().hide().catch(() => {});
  }, [profile, serverRunning, closeToTray]);

  // Tray "Assistant" item: open the tab in the main window.
  useEffect(() => {
    const unlisten = listen("assistant_open", () => setTab("assistant-chat"));
    return () => {
      unlisten.then((f) => f());
    };
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
            setProfileMirror((c.chat_profile ?? "agent") as Profile);
            setCloseToTray(c.close_to_tray ?? false);
            setWebuiEnabled(c.webui_enabled ?? false);
            setOverlayEnabled(c.assistant?.overlay_enabled ?? false);
            if (external) setTab((t) => (t === "run" ? "chat" : t));
          })
          .catch(() => {});
      }),
    [],
  );

  // A disabled profile can never be active.
  useEffect(() => {
    if (!webuiEnabled && profile === "webui") setProfileMirror("agent");
  }, [webuiEnabled, profile]);

  // Every visible tab must belong to the active profile: pages navigate with
  // hard-coded tab ids, so re-check on tab changes too, not just profile flips.
  useEffect(() => {
    setTab((t) => {
      if (!webuiEnabled && t === "webui") return "chat";
      return visibleTab(t, profile, externalMode);
    });
  }, [tab, profile, externalMode, webuiEnabled]);

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
    // Safety net only: a slow dev bundle must not leave the window hidden.
    const fallback = setTimeout(reveal, 10000);
    return () => clearTimeout(fallback);
  }, [wizard]);

  if (wizard === null) return null;
  if (wizard) return <Wizard onDone={() => setWizard(false)} />;

  const shownTab = visibleTab(tab, profile, externalMode);

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
              {TABS.filter((t) => {
                if (t.id === "bench" && !showBench) return false;
                if (t.id === "run" && externalMode) return false;
                if ((t.id === "chat" || t.id === "agent") && profile !== "agent") return false;
                if (t.id === "tools" && (profile === "roleplay" || profile === "assistant")) return false;
                if ((t.id === "roleplay" || t.id === "talk") && profile !== "roleplay") return false;
                if (t.id === "assistant" && profile !== "assistant") return false;
                if (t.id === "assistant-chat" && profile !== "assistant") return false;
                if (t.id === "webui" && (!webuiEnabled || profile !== "webui" || externalMode)) return false;
                return true;
              }).map(({ id, label, icon: Icon }) => {
                // Server state colors Run (green) and the active chat tab.
                const chatTab: Tab =
                  profile === "roleplay"
                    ? "talk"
                    : profile === "assistant"
                      ? "assistant-chat"
                      : "chat";
                const highlight =
                  serverRunning && id === "run"
                    ? "text-accent-green"
                    : serverRunning && id === chatTab
                      ? "text-ink"
                      : null;
                // Filled glyphs while the server is up.
                const filled = serverRunning && (id === "run" || id === chatTab);
                return (
                  <button
                    key={id}
                    onClick={() => {
                      setTab(id);
                      closeSettings();
                    }}
                    className={`flex items-center gap-1.5 px-2.5 py-1 rounded text-xs font-medium transition-colors whitespace-nowrap ${
                      shownTab === id ? "bg-accent/20" : "hover:bg-accent/10"
                    } ${highlight ?? (shownTab === id ? "text-ink" : "text-dim hover:text-ink")}`}
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
            title={updateReady ? "Settings — update available" : "Settings"}
            className={`relative w-9 h-9 shrink-0 inline-flex items-center justify-center transition-colors ${
              settingsOpen ? "bg-accent/20 text-ink" : "text-dim hover:text-ink hover:bg-surface-3"
            }`}
          >
            <SettingsIcon size={14} />
            {updateReady && (
              <span className="absolute top-1.5 right-1.5 h-1.5 w-1.5 rounded-full bg-accent" />
            )}
          </button>
          <WindowControls />
        </div>
      </div>
      <main className="flex-1 min-w-0 min-h-0 relative">
        {/* Chat, Run, Dashboard, Tools and Mode stay mounted off-tab: stream,
            launch, download and form state survive switches. */}
        <div className="h-full min-h-0" style={{ display: shownTab === "chat" ? undefined : "none" }}>
          <Chat go={setTab} active={shownTab === "chat"} />
        </div>
        <div className="h-full min-h-0" style={{ display: shownTab === "talk" ? undefined : "none" }}>
          <Talk go={setTab} active={shownTab === "talk"} />
        </div>
        <div className="h-full min-h-0" style={{ display: shownTab === "assistant" ? undefined : "none" }}>
          <Assistant active={shownTab === "assistant"} />
        </div>
        <div
          className="h-full min-h-0"
          style={{ display: shownTab === "assistant-chat" ? undefined : "none" }}
        >
          <AssistantChat go={setTab} active={shownTab === "assistant-chat"} />
        </div>
        <div className="h-full min-h-0" style={{ display: shownTab === "webui" ? undefined : "none" }}>
          <WebUI go={setTab} active={shownTab === "webui"} />
        </div>
        <div className="h-full min-h-0" style={{ display: shownTab === "run" ? undefined : "none" }}>
          <Run go={setTab} />
        </div>
        <div className="h-full min-h-0" style={{ display: shownTab === "dashboard" ? undefined : "none" }}>
          <Dashboard go={setTab} />
        </div>
        {shownTab === "bench" && showBench && <Bench />}
        <div className="h-full min-h-0" style={{ display: shownTab === "tools" ? undefined : "none" }}>
          <Tools active={shownTab === "tools"} />
        </div>
        <div className="h-full min-h-0" style={{ display: shownTab === "agent" ? undefined : "none" }}>
          <Agent active={shownTab === "agent"} />
        </div>
        <div className="h-full min-h-0" style={{ display: shownTab === "roleplay" ? undefined : "none" }}>
          <Roleplay go={(t) => setTab(t)} />
        </div>
        <div className="h-full min-h-0" style={{ display: shownTab === "mode" ? undefined : "none" }}>
          <Mode go={(t) => setTab(t)} />
        </div>
        <SettingsPanel
          open={settingsOpen}
          onClose={closeSettings}
          section={settingsSection}
          onSectionChange={setSettingsSection}
          onRestartWizard={() => {
            closeSettings();
            setWizard(true);
          }}
        />
      </main>
    </div>
  );
}
