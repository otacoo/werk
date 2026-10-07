import { afterEach, describe, expect, it } from "vitest";
import { getLocale, registerCatalog, setLocale, t } from "./i18n";

afterEach(() => setLocale("en"));

describe("t", () => {
  it("falls back to the source string", () => {
    expect(t("Hello")).toBe("Hello");
  });

  it("interpolates vars and keeps unknown ones visible", () => {
    expect(t("Hi {name}, {n} new", { name: "Ada", n: 3 })).toBe("Hi Ada, 3 new");
    expect(t("Hi {name}")).toBe("Hi {name}");
  });

  it("uses a registered catalog and falls back per key", () => {
    registerCatalog("de", { Hello: "Hallo" });
    setLocale("de");
    expect(t("Hello")).toBe("Hallo");
    expect(t("Other")).toBe("Other");
  });

  it("ignores unknown locales", () => {
    setLocale("xx");
    expect(getLocale()).toBe("en");
  });
});
