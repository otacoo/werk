import { useEffect, useState } from "react";
import { commands } from "../../bindings";
import type { AppConfig } from "../../bindings";
import { call } from "../../utils/ipc";
import Toggle from "../../components/Toggle";

export function BehaviorCard({ appConfig, setAppConfig }: {
  appConfig: AppConfig | null;
  setAppConfig: React.Dispatch<React.SetStateAction<AppConfig | null>>;
}) {
  return (
    <div className="card">
      <h2 className="section-title mb-1">Verification nudges</h2>
      <p className="section-desc">
        Tell the model to verify its claims before committing; especially useful for smaller
        models. Off disables claim checks; thorough also asks to check read claims.
      </p>
      <select
        className="input w-full py-1 px-2 text-xs mt-3"
        value={appConfig?.verify_mode ?? "normal"}
        onChange={async (e) => {
          const mode = e.target.value as "off" | "normal" | "thorough";
          setAppConfig((c) => (c ? { ...c, verify_mode: mode } : c));
          try {
            await call(commands.setVerifyMode(mode));
          } catch {}
        }}
      >
        <option value="off">Off</option>
        <option value="normal">High-stakes only</option>
        <option value="thorough">Thorough</option>
      </select>
    </div>
  );
}

export function UtilityModelCard({ appConfig, setAppConfig }: {
  appConfig: AppConfig | null;
  setAppConfig: React.Dispatch<React.SetStateAction<AppConfig | null>>;
}) {
  const favorites = appConfig?.provider_favorites ?? [];
  const roles: { value: string; label: string }[] = [];
  if (appConfig?.server_mode === "router") {
    if (appConfig.harness_roles?.orchestrator) {
      roles.push({ value: "orchestrator", label: "Local: orchestrator" });
    }
    if (appConfig.harness_roles?.worker) {
      roles.push({ value: "worker", label: "Local: worker" });
    }
  }
  return (
    <div className="card">
      <h2 className="section-title mb-1">Utility model</h2>
      <p className="section-desc">
        Runs housekeeping turns — context compaction and /distill — keeping them off your main
        model. Favorites come from the Mode tab.
      </p>
      <select
        className="input w-full py-1 px-2 text-xs mt-3"
        value={appConfig?.utility_target ?? ""}
        onChange={async (e) => {
          const target = e.target.value || null;
          setAppConfig((c) => (c ? { ...c, utility_target: target } : c));
          try {
            await call(commands.setUtilityTarget(target));
          } catch {}
        }}
      >
        <option value="">Same as the main model</option>
        {roles.map((r) => (
          <option key={r.value} value={r.value}>{r.label}</option>
        ))}
        {favorites.map((t) => (
          <option key={t} value={t}>{t}</option>
        ))}
      </select>
    </div>
  );
}


export function ServerLifecycleCard({ appConfig, refresh }: {
  appConfig: AppConfig | null;
  refresh: () => Promise<void>;
}) {
  const [flash, setFlash] = useState<string | null>(null);
  const [idleDraft, setIdleDraft] = useState("5");
  const autoStart = appConfig?.server_auto_start ?? false;
  const idle = appConfig?.server_idle_unload_minutes ?? 5;

  useEffect(() => setIdleDraft(String(idle)), [idle]);

  const save = async (nextAuto: boolean, nextIdle: number) => {
    try {
      await call(commands.setServerLifecycle(nextAuto, nextIdle));
      await refresh();
      setFlash("Saved");
    } catch (e) {
      setFlash(String(e));
    }
  };

  return (
    <div className="card">
      <h2 className="section-title mb-1">Local server</h2>
      <p className="section-desc">
        Lifecycle for the local llama.cpp server (single, router, and mixed modes). External API
        mode is unaffected, and the idle unload is skipped while the Web UI is enabled.
      </p>
      <div className="mt-3 space-y-3">
        <Toggle
          label="Start automatically when chatting"
          checked={autoStart}
          onChange={(v) => save(v, idle)}
        />
        <label className="label flex items-center gap-2">
          <span title="Stop the server after this many minutes without chat activity; 0 disables.">
            Unload after idle (minutes)
          </span>
          <input
            type="number"
            min={0}
            max={1440}
            className="input w-20 py-0.5 px-1.5 text-xs"
            value={idleDraft}
            onChange={(e) => setIdleDraft(e.target.value)}
            onBlur={() => {
              const n = Math.max(0, Math.min(1440, Number(idleDraft) || 0));
              setIdleDraft(String(n));
              if (n !== idle) save(autoStart, n);
            }}
          />
        </label>
      </div>
      {flash && <p className="text-[0.6875rem] text-accent-green mt-2">{flash}</p>}
    </div>
  );
}

