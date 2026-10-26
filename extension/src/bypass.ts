// On every page: holding Alt while clicking a download link lets the browser download it
// instead of Velo.

document.addEventListener(
  "mousedown",
  (event) => {
    if (event.altKey) void chrome.runtime.sendMessage({ type: "bypass" }).catch(() => undefined);
  },
  true,
);
