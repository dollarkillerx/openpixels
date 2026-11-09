// Converts Lensfun's XML database into one compact JSON file for the browser.
// Usage: node scripts/build-lensfun.mjs <xml directory> <output directory> <commit>
import { mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const [source, target, commit] = process.argv.slice(2);

const decode = (text) => text.replace(/&amp;/g, "&").replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&quot;/g, '"').replace(/&apos;/g, "'").trim();
const texts = (block, tag) => [...block.matchAll(new RegExp(`<${tag}(?:\\s[^>]*)?>([^<]*)</${tag}>`, "g"))].map((match) => decode(match[1]));
const number = (block, tag, fallback) => {
  const value = Number(texts(block, tag)[0]);
  return Number.isFinite(value) && value > 0 ? value : fallback;
};
const attributes = (element) => Object.fromEntries([...element.matchAll(/([\w-]+)="([^"]*)"/g)].map((match) => [match[1], match[2]]));
const elements = (block, tag) => [...block.matchAll(new RegExp(`<${tag}\\s[^>]*/?>`, "g"))].map((match) => attributes(match[0]));
const numbers = (entry, names) => names.map((name) => Number(entry[name] ?? 0));

// Must match `normalize` in web/src/lensfun.ts.
/**
 * What a lens is called, however a camera or database spells it: no maker's name, Sigma's
 * product lines as one letter, no model code, and only letters, digits and points.
 */
const normalize = (text) =>
  text
    .toLowerCase()
    .replace(/\b(sigma|tamron|tokina|samyang|rokinon|viltrox|canon|nikon|sony|fujifilm|fujinon|panasonic|olympus)\b/g, "")
    .replace(/[|[(]\s*(a|art)\s*\d*\s*[\])]?\s*$/g, "a")
    .replace(/[|[(]\s*(c|contemporary)\s*\d*\s*[\])]?\s*$/g, "c")
    .replace(/[|[(]\s*(s|sports)\s*\d*\s*[\])]?\s*$/g, "s")
    .replace(/\(?\b[a-z]\d{3}\)?\s*$/g, "")
    .replace(/[^a-z0-9.]/g, "");

const DISTORTION_PARAMETERS = { ptlens: ["a", "b", "c"], poly3: ["k1"], poly5: ["k1", "k2"] };
const TCA_PARAMETERS = { linear: ["kr", "kb"], poly3: ["vr", "vb", "cr", "cb", "br", "bb"] };

const cameras = {};
const lenses = [];

for (const file of readdirSync(source).filter((name) => name.endsWith(".xml")).sort()) {
  const xml = readFileSync(join(source, file), "utf8").replace(/<!--[\s\S]*?-->/g, "");

  for (const [, block] of xml.matchAll(/<camera>([\s\S]*?)<\/camera>/g)) {
    const crop = number(block, "cropfactor", 0);
    if (!crop) continue;
    const makers = texts(block, "maker");
    for (const model of texts(block, "model")) {
      cameras[normalize(model)] = crop;
      for (const maker of makers) cameras[normalize(`${maker} ${model}`)] = crop;
    }
  }

  for (const [, block] of xml.matchAll(/<lens>([\s\S]*?)<\/lens>/g)) {
    const calibration = block.match(/<calibration>([\s\S]*?)<\/calibration>/)?.[1];
    if (!calibration) continue;
    const distortion = elements(calibration, "distortion")
      .filter((entry) => DISTORTION_PARAMETERS[entry.model])
      .map((entry) => [Number(entry.focal), entry.model, ...numbers(entry, DISTORTION_PARAMETERS[entry.model])]);
    const tca = elements(calibration, "tca")
      .filter((entry) => TCA_PARAMETERS[entry.model])
      .map((entry) => [Number(entry.focal), entry.model, ...numbers(entry, TCA_PARAMETERS[entry.model])]);
    const vignetting = elements(calibration, "vignetting")
      .filter((entry) => entry.model === "pa")
      .map((entry) => numbers(entry, ["focal", "aperture", "distance", "k1", "k2", "k3"]));
    if (distortion.length + tca.length + vignetting.length === 0) continue;
    const makers = texts(block, "maker");
    const names = new Set();
    for (const model of texts(block, "model")) {
      names.add(normalize(model));
      for (const maker of makers) names.add(normalize(`${maker} ${model}`));
    }
    lenses.push({
      maker: makers[0] ?? "",
      names: [...names],
      crop: number(block, "cropfactor", 1),
      aspect: number(block, "aspect-ratio", 1.5),
      distortion,
      tca,
      vignetting,
    });
  }
}

mkdirSync(target, { recursive: true });
writeFileSync(join(target, "lensfun.json"), JSON.stringify({ cameras, lenses }));
writeFileSync(
  join(target, "NOTICE.txt"),
  `lensfun.json is converted from the Lensfun lens database, by the Lensfun contributors:
https://github.com/lensfun/lensfun (data/db at commit ${commit})

The conversion keeps the calibration data and changes its file format only.

It is licensed under the Creative Commons Attribution-Share Alike 3.0 license:
https://creativecommons.org/licenses/by-sa/3.0/
`,
);
console.log(`${lenses.length} lenses, ${Object.keys(cameras).length} camera names`);
