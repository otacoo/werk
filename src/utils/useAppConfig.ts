import { useEffect, useState } from "react";
import { commands } from "../bindings";
import type { AppConfig } from "../bindings";
import { call } from "./ipc";
import { setQuickBench } from "./appSettings";

/// Shared snapshot of the backend config. `refresh` refetches on demand;
/// `open` refetches whenever the surface becomes visible.
export function useAppConfig(open: boolean) {
  const [appConfig, setAppConfig] = useState<AppConfig | null>(null);
  const [configError, setConfigError] = useState<string | null>(null);
  const refresh = async () => {
    setConfigError(null);
    try {
      const c = await call(commands.getConfig());
      setAppConfig(c);
      // Feed the live store so the header and Run card follow the toggle.
      setQuickBench(c.bench_visible ?? true);
    } catch (e) {
      setConfigError(String(e));
    }
  };
  useEffect(() => {
    if (open) void refresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);
  return [appConfig, setAppConfig, configError, refresh] as const;
}
