import { useEffect, useRef, useState } from "react";
import { ExternalLink, Globe, Play } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { ServerStatus } from "../bindings";
import { getServerStatus, subscribeServerStatus } from "../utils/serverStatus";
import { t } from "../utils/i18n";
import type { Tab } from "../App";

/// llama-server's own chat UI, embedded. The frame only lives while the tab
/// is on screen and the server is running: the web UI polls the server
/// continuously, so leaving it mounted in the background burns CPU.
export default function WebUI({ go, active = false }: { go: (t: Tab) => void; active?: boolean }) {
  const [status, setStatus] = useState<ServerStatus>(getServerStatus());
  const [url, setUrl] = useState<string | null>(null);
  const loadedRef = useRef(false);

  useEffect(() => subscribeServerStatus(setStatus), []);

  useEffect(() => {
    if (status.type === "running") {
      setUrl(`http://127.0.0.1:${status.port}/`);
      loadedRef.current = true;
    }
  }, [status]);

  const live = status.type === "running" && url !== null;

  if (!loadedRef.current) {
    return (
      <div className="h-full flex flex-col items-center justify-center gap-2 text-center px-6">
        <Globe size={22} className="text-faint" />
        <p className="text-sm text-dim">{t("The local server is not running.")}</p>
        <p className="text-[0.6875rem] text-faint max-w-md">
          {t("Start it on the Run page.")}
        </p>
        <button className="btn-primary text-xs py-1 px-2 mt-1" onClick={() => go("run")}>
          <Play size={11} /> {t("Go to Run")}
        </button>
      </div>
    );
  }

  return (
    <div className="h-full flex flex-col">
      <div className="flex items-center gap-2 px-4 py-1.5 border-b border-border text-[0.6875rem] text-dim shrink-0">
        <Globe size={11} className="shrink-0" />
        <span className="font-mono truncate">{url}</span>
        <button
          className="ml-auto shrink-0 text-accent hover:underline inline-flex items-center gap-1"
          onClick={() => url && void openUrl(url).catch(() => {})}
        >
          <ExternalLink size={11} /> {t("Browser")}
        </button>
      </div>
      {!live && (
        <div className="px-4 py-1.5 border-b border-border text-[0.6875rem] text-accent-yellow bg-accent-yellow/5">
          {t("The server stopped — the frame reloads when it runs again.")}
        </div>
      )}
      {live && active ? (
        <iframe
          src={url ?? undefined}
          className="flex-1 w-full bg-surface-0"
          title="llama-server Web UI"
        />
      ) : (
        <div className="flex-1 flex items-center justify-center px-4 text-center text-xs text-faint">
          {live
            ? t("The web UI runs only while this tab is open.")
            : t("The web UI is unloaded while the server is stopped.")}
        </div>
      )}
    </div>
  );
}
