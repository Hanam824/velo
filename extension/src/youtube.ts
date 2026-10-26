// On YouTube: a "Download" button at the top right of the player that lists the video's
// formats and hands the chosen one to Velo.
//
// The button lives in a closed shadow root, so YouTube's styles and Velo's don't mix.

import { type Answer, type Formats, type PageRequest, failure } from "./protocol.ts";
import { videoId } from "./youtube-id.ts";

const ELEMENT_ID = "velo-download";

const STYLE = `
:host { all: initial; }
.box { font: 14px/1.4 Roboto, Arial, sans-serif; position: relative; }
button.open {
  display: flex; align-items: center; gap: 6px; cursor: pointer;
  padding: 6px 12px; border: 0; border-radius: 18px;
  background: #1f6feb; color: #ffffff; font: inherit; font-weight: 500;
  box-shadow: 0 1px 4px rgba(0, 0, 0, 0.4); opacity: 0.92;
}
button.open:hover { opacity: 1; }
.menu {
  position: absolute; top: 40px; right: 0; min-width: 300px; max-width: 420px;
  max-height: 320px; overflow-y: auto; padding: 6px 0; border-radius: 10px;
  background: var(--bg); color: var(--fg); box-shadow: 0 4px 16px rgba(0, 0, 0, 0.45);
}
.menu[hidden] { display: none; }
.title { padding: 6px 14px 8px; color: var(--muted); font-size: 12px; overflow: hidden;
  text-overflow: ellipsis; white-space: nowrap; }
.item { display: block; width: 100%; padding: 8px 14px; border: 0; background: none;
  color: inherit; font: inherit; text-align: left; cursor: pointer; }
.item:hover, .item:focus { background: var(--hover); outline: none; }
.note { padding: 8px 14px; color: var(--muted); }
.note.error { color: var(--error); }
.light { --bg: #ffffff; --fg: #1f2328; --muted: #59636e; --hover: #eef2f6; --error: #cf222e; }
.dark { --bg: #212121; --fg: #f1f1f1; --muted: #aaaaaa; --hover: #3a3a3a; --error: #ff7b72; }
`;

/** Sends a request to the extension's background and waits for the host's answer. */
function ask(request: PageRequest): Promise<Answer | undefined> {
  return chrome.runtime.sendMessage(request).catch(() => undefined);
}

/** Whether YouTube is showing its dark theme. */
function dark(): boolean {
  return (
    document.documentElement.hasAttribute("dark") ||
    window.matchMedia("(prefers-color-scheme: dark)").matches
  );
}

/** Builds the button and its menu for the video at the current address. */
function build(): HTMLElement {
  const element = document.createElement("div");
  element.id = ELEMENT_ID;
  element.style.cssText = "position:absolute;top:12px;right:12px;z-index:70;";
  const root = element.attachShadow({ mode: "closed" });
  const style = document.createElement("style");
  style.textContent = STYLE;
  const box = document.createElement("div");
  box.className = "box";
  const open = document.createElement("button");
  open.className = "open";
  open.textContent = "⬇ Download";
  open.title = "Download with Velo";
  const menu = document.createElement("div");
  menu.className = "menu";
  menu.hidden = true;
  box.append(open, menu);
  root.append(style, box);

  const note = (text: string, error = false) => {
    const line = document.createElement("div");
    line.className = error ? "note error" : "note";
    line.textContent = text;
    menu.replaceChildren(line);
  };
  const close = () => {
    menu.hidden = true;
  };

  open.addEventListener("click", async (event) => {
    event.stopPropagation();
    if (!menu.hidden) return close();
    menu.className = `menu ${dark() ? "dark" : "light"}`;
    menu.hidden = false;
    note("Getting the formats…");
    const url = location.href;
    const answer = await ask({ type: "list_formats", url });
    const error = failure(answer);
    if (error !== undefined || answer?.type !== "formats") {
      return note(error ?? "Velo didn't answer. Is Velo Download Manager installed?", true);
    }
    show(answer, url);
  });

  const show = (formats: Formats, url: string) => {
    const title = document.createElement("div");
    title.className = "title";
    title.textContent = formats.title;
    const items = formats.choices.map((choice) => {
      const item = document.createElement("button");
      item.className = "item";
      item.textContent = choice.label;
      item.addEventListener("click", async (event) => {
        event.stopPropagation();
        note("Sending to Velo…");
        const answer = await ask({ type: "download_format", url, itags: choice.itags });
        const error = failure(answer);
        if (error !== undefined) return note(error, true);
        note("Velo is downloading it.");
        setTimeout(close, 1500);
      });
      return item;
    });
    if (items.length === 0) return note("YouTube offers no format Velo can download.", true);
    menu.replaceChildren(title, ...items);
  };

  document.addEventListener("click", close);
  document.addEventListener("keydown", (event) => {
    if (event.key === "Escape") close();
  });
  return element;
}

/** Adds the button to the player of a video page, or removes it elsewhere. */
function place(): void {
  const existing = document.getElementById(ELEMENT_ID);
  if (videoId(location.href) === undefined) {
    existing?.remove();
    return;
  }
  const player = document.querySelector<HTMLElement>("#movie_player, .html5-video-player");
  if (player === null || existing?.parentElement === player) return;
  existing?.remove();
  player.append(build());
}

// YouTube changes pages without reloading; it says so with this event.
document.addEventListener("yt-navigate-finish", place);
// The player may appear after the page; look again when the page changes, at most once a frame.
let pending = false;
new MutationObserver(() => {
  if (pending) return;
  pending = true;
  requestAnimationFrame(() => {
    pending = false;
    place();
  });
}).observe(document.documentElement, { childList: true, subtree: true });
place();
