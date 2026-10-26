// On every page: holding Alt while clicking a download link lets the browser download it
// instead of Velo.

import { ext } from "./api.ts";

document.addEventListener(
  "mousedown",
  (event) => {
    if (event.altKey) void ext.runtime.sendMessage({ type: "bypass" }).catch(() => undefined);
  },
  true,
);
