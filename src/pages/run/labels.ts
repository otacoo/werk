/// Sentinel name of the built-in default preset (matches the backend).
export const DEFAULT_PRESET = "__default__";

/// Friendly label for the built-in default preset.
export const presetLabel = (name: string) =>
  name === DEFAULT_PRESET ? "Default preset" : name;
