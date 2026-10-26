import assert from "node:assert/strict";
import { test } from "node:test";

import { baseName, cookieHeader, fileExtension, shouldCapture } from "../src/capture.ts";

test("extensions come from file names and addresses", () => {
  assert.equal(fileExtension("/Users/me/Downloads/Setup.EXE"), "exe");
  assert.equal(fileExtension("C:\\Users\\me\\Downloads\\a.tar.gz"), "gz");
  assert.equal(fileExtension("https://a.test/dl/file.zip?token=1#x"), "zip");
  assert.equal(fileExtension("https://a.test/"), undefined);
  assert.equal(fileExtension(".hidden"), undefined);
  assert.equal(fileExtension("name."), undefined);
});

test("only http downloads of listed types are captured, unless Alt is held", () => {
  const extensions = ["zip", "iso"];
  const zip = { url: "https://a.test/get?id=4", filename: "/d/file.ZIP" };
  assert.equal(shouldCapture(zip, extensions, false), true);
  assert.equal(shouldCapture(zip, extensions, true), false);
  assert.equal(shouldCapture({ url: "https://a.test/a.iso", filename: "" }, extensions, false), true);
  assert.equal(shouldCapture({ url: "https://a.test/page.html", filename: "" }, extensions, false), false);
  assert.equal(shouldCapture({ url: "blob:https://a.test/1", filename: "/d/x.zip" }, extensions, false), false);
  assert.equal(shouldCapture({ url: "data:text/plain,hi", filename: "/d/x.zip" }, extensions, false), false);
});

test("cookies become one header", () => {
  assert.equal(cookieHeader([]), undefined);
  assert.equal(
    cookieHeader([
      { name: "a", value: "1" },
      { name: "b", value: "x y" },
    ]),
    "a=1; b=x y",
  );
  assert.equal(baseName("/Users/me/Downloads/report.pdf"), "report.pdf");
  assert.equal(baseName("C:\\d\\r.pdf"), "r.pdf");
  assert.equal(baseName(""), undefined);
});
