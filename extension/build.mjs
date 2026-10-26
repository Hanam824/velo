// Builds the extension for each browser into dist/<browser>/: the bundled scripts, the
// browser's manifest (with the version from package.json) and the icons.
//
//   node build.mjs            # every browser in manifests/
//   node build.mjs chromium   # one browser

import { cpSync, mkdirSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";

import { build } from "esbuild";

const { version } = JSON.parse(readFileSync("package.json", "utf8"));
const browsers = process.argv.length > 2
  ? process.argv.slice(2)
  : readdirSync("manifests").map((name) => name.replace(/\.json$/, ""));

for (const browser of browsers) {
  const out = `dist/${browser}`;
  rmSync(out, { recursive: true, force: true });
  mkdirSync(out, { recursive: true });
  const common = { bundle: true, target: ["chrome110", "firefox115", "safari16"], logLevel: "warning" };
  // The background is a module; content scripts run as plain scripts.
  await build({ ...common, entryPoints: ["src/background.ts"], outfile: `${out}/background.js`, format: "esm" });
  await build({
    ...common,
    entryPoints: ["src/youtube.ts", "src/bypass.ts"],
    outdir: out,
    format: "iife",
  });
  const manifest = JSON.parse(readFileSync(`manifests/${browser}.json`, "utf8"));
  manifest.version = version;
  writeFileSync(`${out}/manifest.json`, `${JSON.stringify(manifest, null, 2)}\n`);
  cpSync("icons", `${out}/icons`, { recursive: true });
  console.log(`built ${out}`);
}
