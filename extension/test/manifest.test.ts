import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { test } from "node:test";

const manifest = (browser: string) => JSON.parse(readFileSync(`manifests/${browser}.json`, "utf8"));

test("every browser gets the same permissions and scripts", () => {
  const chromium = manifest("chromium");
  const firefox = manifest("firefox");
  assert.deepEqual(firefox.permissions, chromium.permissions);
  assert.deepEqual(firefox.host_permissions, chromium.host_permissions);
  assert.deepEqual(firefox.content_scripts, chromium.content_scripts);
  assert.equal(firefox.background.scripts[0], chromium.background.service_worker);
  assert.equal(firefox.browser_specific_settings.gecko.id, "velo@hanam824.github.io");
  assert.equal(firefox.key, undefined, "Firefox has no key field");
});

test("the Chromium key gives the ID that Velo's host allows", () => {
  const key = Buffer.from(manifest("chromium").key, "base64");
  const id = [...createHash("sha256").update(key).digest("hex").slice(0, 32)]
    .map((digit) => String.fromCharCode(97 + Number.parseInt(digit, 16)))
    .join("");
  assert.equal(id, "knncpmilkobjkfcjafpleodokkhcaing");
});
