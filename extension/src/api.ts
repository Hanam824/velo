// The extension API: Firefox and Safari offer it as `browser`, with promises; Chromium as
// `chrome`, which also returns promises in Manifest V3. Both have the same shape here.

export const ext: typeof chrome = (globalThis as { browser?: typeof chrome }).browser ?? chrome;
