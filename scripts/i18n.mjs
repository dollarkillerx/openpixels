#!/usr/bin/env node
// Collects every interface text from both editions and checks each translation has it.
//
//   node scripts/i18n.mjs            report missing and unused translations; exit 1 if any are missing
//   node scripts/i18n.mjs --keys     print every key, one JSON string per line
//   node scripts/i18n.mjs --prune    also drop unused translations from the language files
//
// Keys are the Simplified Chinese text: t("…") in web/src, I18n.T("…") in the desktop's C#,
// and {local:Tr '…'} in its XAML. Translations live in web/src/i18n/{language}.json.

import { readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
export const LANGUAGES = ["en", "ja", "zh-TW", "ko"];
const LOCALES = join(root, "web/src/i18n");

function files(folder, extensions) {
  const out = [];
  for (const name of readdirSync(folder)) {
    if (["node_modules", "bin", "obj", "wasm-pkg", "i18n"].includes(name)) continue;
    const path = join(folder, name);
    if (statSync(path).isDirectory()) out.push(...files(path, extensions));
    else if (extensions.some((extension) => name.endsWith(extension))) out.push(path);
  }
  return out;
}

/** Undoes the escapes a C-like double-quoted literal may contain. */
const unescape = (text) => JSON.parse(`"${text}"`);

export function collectKeys() {
  const keys = new Map();
  const add = (key, file) => {
    if (!keys.has(key)) keys.set(key, new Set());
    keys.get(key).add(file.slice(root.length + 1));
  };
  for (const file of files(join(root, "web/src"), [".ts", ".tsx"])) {
    if (file.endsWith("i18n.ts")) continue;
    const source = readFileSync(file, "utf8");
    for (const match of source.matchAll(/\bt\(\s*"((?:[^"\\\n]|\\.)*)"/g)) add(unescape(match[1]), file);
  }
  const desktop = join(root, "desktop/OpenPixels.Desktop");
  for (const file of files(desktop, [".cs"])) {
    if (file.endsWith("I18n.cs")) continue;
    const source = readFileSync(file, "utf8");
    for (const match of source.matchAll(/\bI18n\.T\(\s*"((?:[^"\\\n]|\\.)*)"/g)) add(unescape(match[1]), file);
  }
  for (const file of files(desktop, [".axaml"])) {
    const source = readFileSync(file, "utf8");
    for (const match of source.matchAll(/\{local:Tr '([^']*)'\}/g)) add(match[1], file);
  }
  return keys;
}

export function loadLanguage(language) {
  return JSON.parse(readFileSync(join(LOCALES, `${language}.json`), "utf8"));
}

/** Missing and unused keys per language. */
export function check() {
  const keys = collectKeys();
  return LANGUAGES.map((language) => {
    const table = loadLanguage(language);
    const missing = [...keys.keys()].filter((key) => typeof table[key] !== "string" || table[key].trim() === "");
    const unused = Object.keys(table).filter((key) => !keys.has(key));
    // A translation must keep the original's placeholders, or values would go missing.
    const placeholders = (text) => [...text.matchAll(/\{(\w+)\}/g)].map((match) => match[1]).sort().join(",");
    const broken = [...keys.keys()].filter((key) => typeof table[key] === "string" && placeholders(table[key]) !== placeholders(key));
    return { language, missing, unused, broken };
  });
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  if (process.argv.includes("--keys")) {
    for (const key of [...collectKeys().keys()].sort()) console.log(JSON.stringify(key));
    process.exit(0);
  }
  const keys = collectKeys();
  let failed = false;
  for (const { language, missing, unused, broken } of check()) {
    console.log(`${language}: ${keys.size - missing.length}/${keys.size} translated, ${unused.length} unused, ${broken.length} with wrong placeholders`);
    for (const key of missing.slice(0, 20)) console.log(`  missing: ${JSON.stringify(key)}  (${[...keys.get(key)].join(", ")})`);
    if (missing.length > 20) console.log(`  … and ${missing.length - 20} more`);
    for (const key of broken) console.log(`  placeholders differ: ${JSON.stringify(key)}`);
    if (process.argv.includes("--prune") && unused.length > 0) {
      const table = loadLanguage(language);
      for (const key of unused) delete table[key];
      writeFileSync(join(LOCALES, `${language}.json`), `${JSON.stringify(table, null, 2)}\n`);
    }
    failed ||= missing.length > 0 || broken.length > 0;
  }
  process.exit(failed ? 1 : 0);
}
