import { getCurrentWindow } from "@tauri-apps/api/window";

export default function WindowControls({ tall = false }: { tall?: boolean }) {
  const win = getCurrentWindow();
  const run = (fn: () => Promise<void>) => () => {
    fn().catch((e) => console.error("Window control failed:", e));
  };
  const btn = `w-11 ${
    tall ? "h-full" : "h-9"
  } inline-flex items-center justify-center text-dim hover:text-ink hover:bg-surface-3 transition-colors`;
  return (
    <div className={`flex items-stretch shrink-0 ${tall ? "" : "-mt-px -mr-px"}`}>
      <button className={btn} title="Minimize" onClick={run(() => win.minimize())}>
        <span className="text-sm leading-none">−</span>
      </button>
      <button
        className={btn}
        title="Maximize / restore"
        onClick={run(() => win.toggleMaximize())}
      >
        <span className="text-xs leading-none">▢</span>
      </button>
      <button
        className={`${btn} hover:!bg-accent-red hover:!text-white`}
        title="Close"
        onClick={run(() => win.close())}
      >
        <span className="text-sm leading-none">×</span>
      </button>
    </div>
  );
}
