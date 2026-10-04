import { useEffect, useState } from "react";
import { Terminal } from "lucide-react";
import { commands } from "../bindings";
import type { ServerStatus } from "../bindings";
import { call } from "../utils/ipc";
import ApiCard from "../components/ApiCard";

/// API info pane, hosted under Tools.
export function ApiPane() {
  const [status, setStatus] = useState<ServerStatus>({ type: "stopped" });

  useEffect(() => {
    const poll = () =>
      call(commands.getServerStatus()).then(setStatus).catch(() => {});
    poll();
    const id = setInterval(poll, 2000);
    return () => clearInterval(id);
  }, []);

  const port = status.type === "running" ? status.port : null;

  return (
    <>
      <div className="card">
        <h2 className="section-title mb-1">Access</h2>
        <p className="section-desc">
          To require authentication, set an API key in Run → Launch options → Network → API
          key. llama-server then rejects unauthenticated requests, and werk's own chat and
          status probes carry the key automatically.
        </p>
        <div className="mt-2 flex items-center gap-2 text-[0.6875rem] text-faint">
          <Terminal size={12} />
          <span className="font-mono">
            curl -H "Authorization: Bearer &lt;key&gt;" {port != null ? `http://127.0.0.1:${port}/v1/models` : "http://127.0.0.1:8080/v1/models"}
          </span>
        </div>
      </div>

      <ApiCard />
    </>
  );
}
