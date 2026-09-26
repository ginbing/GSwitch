import { beforeEach, describe, expect, it } from "vitest";

import {
  LANGUAGE_PREFERENCE_KEY,
  formatCompactExpiry,
  formatDateTimeWithRelative,
  readLanguagePreference,
  resolveLocale,
  saveLanguagePreference,
} from "./i18n";

describe("GSwitch language settings", () => {
  beforeEach(() => {
    window.localStorage.clear();
  });

  it("uses English for unsupported system locales and Simplified Chinese for supported Chinese locales", () => {
    expect(resolveLocale("system", "fr-FR")).toEqual({ language: "en", formatLocale: "en-US" });
    expect(resolveLocale("system", "zh-CN")).toEqual({ language: "zh-CN", formatLocale: "zh-CN" });
    expect(resolveLocale("en", "zh-CN")).toEqual({ language: "en", formatLocale: "en-US" });
    expect(resolveLocale("system", "zh-TW")).toEqual({ language: "en", formatLocale: "en-US" });
  });

  it("persists a manual language choice without storing account data", () => {
    expect(readLanguagePreference()).toBe("system");

    saveLanguagePreference("zh-CN");

    expect(readLanguagePreference()).toBe("zh-CN");
    expect(window.localStorage).toHaveLength(1);
    expect(window.localStorage.getItem(LANGUAGE_PREFERENCE_KEY)).toBe("zh-CN");
  });

  it("formats reset times with the selected locale and relative time", () => {
    const timestamp = Date.UTC(2026, 0, 2, 12, 0, 0) / 1000;
    const nowMs = Date.UTC(2026, 0, 2, 11, 0, 0);
    const expectedDate = new Intl.DateTimeFormat("zh-CN", {
      dateStyle: "medium",
      timeStyle: "short",
    }).format(new Date(timestamp * 1000));
    const expectedRelative = new Intl.RelativeTimeFormat("zh-CN", { numeric: "auto" }).format(1, "hour");

    expect(formatDateTimeWithRelative(timestamp, "zh-CN", nowMs)).toBe(
      `${expectedDate} (${expectedRelative})`,
    );
  });

  it("keeps credit expiry compact while showing the year only across years", () => {
    const now = new Date(2026, 8, 26, 10, 0).getTime();
    const thisYear = new Date(2026, 9, 4, 13, 0).getTime() / 1000;
    const nextYear = new Date(2027, 0, 1, 10, 0).getTime() / 1000;
    expect(formatCompactExpiry(thisYear, "zh-CN", now)).toMatch(/^8天3小时后 · 10\/04 13:00$/);
    expect(formatCompactExpiry(nextYear, "en-US", now)).toContain("2027/01/01 10:00");
    expect(formatCompactExpiry(now / 1000 - 60, "zh-CN", now)).toBe("已到期");
  });
});
