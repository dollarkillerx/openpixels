/**
 * Interface languages. The Simplified Chinese text is itself the key: `t("打开照片")`.
 * Each other language has a JSON file in ./i18n mapping that text to a translation. The
 * desktop edition reads the same files, so both editions say the same thing.
 *
 * Placeholders are named, as in `t("已导出 {count} 张", { count })`, and stay in the
 * translations as written.
 */

export type Language = "zh-CN" | "zh-TW" | "en" | "ja" | "ko";

export const LANGUAGES: { code: Language; name: string }[] = [
  { code: "zh-CN", name: "简体中文" },
  { code: "zh-TW", name: "繁體中文" },
  { code: "en", name: "English" },
  { code: "ja", name: "日本語" },
  { code: "ko", name: "한국어" },
];

const STORAGE_KEY = "openpixels.language.v1";
/** Used for any text a language has no translation for yet. */
const FALLBACK = "en" satisfies Language;

type Table = Record<string, string>;

const loaders: Record<Exclude<Language, "zh-CN">, () => Promise<{ default: Table }>> = {
  "zh-TW": () => import("./i18n/zh-TW.json"),
  en: () => import("./i18n/en.json"),
  ja: () => import("./i18n/ja.json"),
  ko: () => import("./i18n/ko.json"),
};

let current: Language = "zh-CN";
let table: Table | null = null;
let fallback: Table | null = null;

const isLanguage = (value: unknown): value is Language => LANGUAGES.some((item) => item.code === value);

/** The best match for a BCP 47 tag, or null. Any Chinese that is not Traditional is Simplified. */
export function matchLanguage(tag: string): Language | null {
  const lower = tag.toLowerCase();
  if (lower.startsWith("zh")) return /hant|tw|hk|mo/.test(lower) ? "zh-TW" : "zh-CN";
  if (lower.startsWith("ja")) return "ja";
  if (lower.startsWith("ko")) return "ko";
  if (lower.startsWith("en")) return "en";
  return null;
}

/** ?lang= first, so links and tests can choose; then the saved choice; then the browser's. */
function chosenLanguage(): Language {
  try {
    const requested = new URLSearchParams(globalThis.location?.search ?? "").get("lang");
    if (isLanguage(requested)) return requested;
    const saved = globalThis.localStorage?.getItem(STORAGE_KEY);
    if (isLanguage(saved)) return saved;
  } catch {
    // Storage can be blocked; the browser's languages still work.
  }
  for (const tag of globalThis.navigator?.languages ?? []) {
    const match = matchLanguage(tag);
    if (match) return match;
  }
  return globalThis.navigator ? FALLBACK : "zh-CN";
}

/** Loads the interface language. Call once before rendering anything that uses `t`. */
export async function initI18n(language: Language = chosenLanguage()) {
  current = language;
  table = language === "zh-CN" ? null : (await loaders[language]()).default;
  fallback = language === "zh-CN" || language === FALLBACK ? null : (await loaders[FALLBACK]()).default;
  if (globalThis.document) document.documentElement.lang = language;
}

export const currentLanguage = () => current;

/** Saves a new interface language and reloads, so every module picks it up. */
export function setLanguage(language: Language) {
  try {
    localStorage.setItem(STORAGE_KEY, language);
  } catch {
    // Without storage the choice lasts until the page is left.
  }
  const url = new URL(location.href);
  url.searchParams.delete("lang");
  if (language !== current) location.assign(url);
}

/** Text in the interface language, with `{name}` placeholders filled in. */
export function t(text: string, params?: Record<string, string | number>): string {
  const translated = table?.[text] ?? fallback?.[text] ?? text;
  if (!params) return translated;
  return translated.replace(/\{(\w+)\}/g, (whole, name: string) => (name in params ? String(params[name]) : whole));
}
