export type LaunchTab = "context" | "hardware" | "sampling" | "network" | "chat" | "extra";

export interface SettingMatch {
  el: HTMLElement;
  text: string;
  tab: LaunchTab;
}

/// Indexes the launch-option labels so the search box can jump to them.
export function indexSettings(root: HTMLElement, q: string): SettingMatch[] {
  const results: SettingMatch[] = [];
  const seen = new Set<HTMLElement>();
  const push = (el: HTMLElement) => {
    if (seen.has(el)) return;
    // Mode-gated options are display:none; never index what can't be shown.
    if (el.offsetParent === null) return;
    seen.add(el);
    let text = el.textContent?.trim() ?? "";
    const titled = el.querySelector<HTMLElement>("[title]");
    if (titled?.title) text += " " + titled.title;
    const hint = el.nextElementSibling;
    if (hint && hint.tagName === "P") text += " " + (hint.textContent?.trim() ?? "");
    if (!text.toLowerCase().includes(q)) return;
    const pane = el.closest<HTMLElement>("[data-launch-tab]");
    const tab = (pane?.dataset.launchTab as LaunchTab | undefined) ?? "context";
    results.push({ el, text, tab });
  };
  root.querySelectorAll<HTMLElement>(".label").forEach(push);
  root.querySelectorAll<HTMLElement>('p[class*="font-medium"]').forEach(push);
  root.querySelectorAll<HTMLElement>('p[class*="font-semibold"]').forEach(push);
  root.querySelectorAll<HTMLElement>('p[class*="font-bold"]').forEach(push);
  return results;
}

