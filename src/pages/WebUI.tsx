import { useEffect, useRef, useState } from "react";
import { ExternalLink, Globe, Play } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import type { ServerStatus } from "../bindings";
import { getServerStatus, subscribeServerStatus } from "../utils/serverStatus";
import type { Tab } from "../App";

/// llama-server's own chat UI, embedded. Mounted while the app runs so
/// switching tabs never reloads it.
export default function WebUI({ go }: { go: (t: Tab) => void; active?: boolean }) {
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
        <p className="text-sm text-dim">The local server is not running.</p>
        <p className="text-[0.6875rem] text-faint max-w-md">
          Start it on the Run page — the WebUI profile enables the server's own UI automatically.
          The embedded UI talks to the model with the server's own template; werk's prompt and
          tools don't apply.
        </p>
        <button className="btn-primary text-xs py-1 px-2 mt-1" onClick={() => go("run")}>
          <Play size={11} /> Go to Run
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
          <ExternalLink size={11} /> Browser
        </button>
      </div>
      {!live && (
        <div className="px-4 py-1.5 border-b border-border text-[0.6875rem] text-accent-yellow bg-accent-yellow/5">
          The server stopped — the frame below shows its last state.
        </div>
      )}
      <iframe
        src={url ?? undefined}
        className="flex-1 w-full bg-surface-0"
        title="llama-server Web UI"
      />
    </div>
  );
}
