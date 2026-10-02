//! In-memory mirror of hot backend config: surfaces react to toggles
//! immediately instead of waiting for Settings to close.

let quickBench = false;
const quickBenchListeners = new Set<(v: boolean) => void>();

// Chat profile mirror: the Mode page flips it and the header reacts at once,
// without waiting for the config refetch.
let profile = "agent";
const profileListeners = new Set<(p: string) => void>();

export function getProfile(): string {
  return profile;
}

export function setProfileMirror(p: string): void {
  if (p === profile) return;
  profile = p;
  profileListeners.forEach((l) => l(p));
}

export function subscribeProfile(cb: (p: string) => void): () => void {
  profileListeners.add(cb);
  return () => {
    profileListeners.delete(cb);
  };
}

// Cross-page "backend config changed" signal: pages that keep a local config
// snapshot (Run) refetch instead of showing stale state.
const configListeners = new Set<() => void>();

export function notifyConfigChanged(): void {
  configListeners.forEach((l) => l());
}

export function subscribeConfigChanged(cb: () => void): () => void {
  configListeners.add(cb);
  return () => {
    configListeners.delete(cb);
  };
}

export function getQuickBench(): boolean {
  return quickBench;
}

export function setQuickBench(v: boolean): void {
  if (v === quickBench) return;
  quickBench = v;
  quickBenchListeners.forEach((l) => l(v));
}

export function subscribeQuickBench(cb: (v: boolean) => void): () => void {
  quickBenchListeners.add(cb);
  return () => {
    quickBenchListeners.delete(cb);
  };
}
