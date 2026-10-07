//! Tiny i18n layer: user-facing strings call `t("English source")`. A locale
//! adds a catalog keyed by that source string; anything untranslated falls
//! back to the source, so adoption can be incremental (translate as we go).

type Vars = Record<string, string | number>;

const catalogs: Record<string, Record<string, string>> = {};

let locale = "en";

export function getLocale(): string {
  return locale;
}

export function setLocale(next: string): void {
  locale = next === "en" || catalogs[next] ? next : "en";
}

export function t(source: string, vars?: Vars): string {
  const template = catalogs[locale]?.[source] ?? source;
  if (!vars) return template;
  return template.replace(/\{(\w+)\}/g, (_, name: string) =>
    name in vars ? String(vars[name]) : `{${name}}`,
  );
}

/// Register (or replace) a locale catalog. English is the source language, so
/// it needs no catalog; other locales key their entries by the source string.
export function registerCatalog(name: string, entries: Record<string, string>): void {
  catalogs[name] = entries;
}
