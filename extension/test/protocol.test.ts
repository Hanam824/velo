import assert from "node:assert/strict";
import { test } from "node:test";

import { failure, isCompatible, PROTOCOL } from "../src/protocol.ts";
import { videoId } from "../src/youtube-id.ts";

test("only a host of this protocol version is used", () => {
  assert.equal(isCompatible({ type: "hello", protocol: PROTOCOL, app_version: "0.1.0", capture_extensions: [] }), true);
  assert.equal(isCompatible({ type: "hello", protocol: PROTOCOL + 1, capture_extensions: [] }), false);
  assert.equal(isCompatible({ type: "error", error: "x" }), false);
  assert.equal(isCompatible(undefined), false);
});

test("answers report their failure", () => {
  assert.equal(failure({ type: "result", ok: true }), undefined);
  assert.equal(failure({ type: "result", ok: false, error: "no" }), "no");
  assert.equal(failure({ type: "error", error: "broken" }), "broken");
  assert.equal(failure(undefined), "Velo didn't answer");
});

test("video IDs are found as the app finds them", () => {
  for (const address of [
    "https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=4",
    "https://m.youtube.com/watch?v=dQw4w9WgXcQ",
    "https://www.youtube.com/shorts/dQw4w9WgXcQ",
    "https://youtu.be/dQw4w9WgXcQ",
    "https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ",
  ]) {
    assert.equal(videoId(address), "dQw4w9WgXcQ", address);
  }
  for (const address of ["https://www.youtube.com/", "https://www.youtube.com/@channel", "https://example.com/watch?v=dQw4w9WgXcQ", "nonsense"]) {
    assert.equal(videoId(address), undefined, address);
  }
});
