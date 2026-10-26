// The extension's background: talks to Velo through the native-messaging host, takes over
// downloads of the file types Velo captures, adds "Download with Velo" to link menus, and
// answers the YouTube button's questions.

import { ext } from "./api.ts";
import { baseName, cookieHeader, shouldCapture } from "./capture.ts";
import { type Answer, HOST, type PageRequest, type Request, failure, isCompatible } from "./protocol.ts";

/** How long holding Alt lets downloads go to the browser instead of Velo. */
const BYPASS_MS = 3000;

/** The capture list, refreshed from the host this often. */
const REFRESH_MS = 60 * 60 * 1000;

/** What the background keeps between events; a service worker can stop at any time. */
interface State {
  /** Whether capturing is on (the toolbar button turns it off). */
  enabled: boolean;
  /** File types to capture, from the host; empty while the host can't be reached. */
  extensions: string[];
  /** When `extensions` was last asked for. */
  checkedAt: number;
  /** Until when downloads go to the browser because Alt was held. */
  bypassUntil: number;
}

const DEFAULT_STATE: State = { enabled: true, extensions: [], checkedAt: 0, bypassUntil: 0 };

async function state(): Promise<State> {
  const stored = await ext.storage.local.get("state");
  return { ...DEFAULT_STATE, ...(stored.state as Partial<State> | undefined) };
}

async function update(change: Partial<State>): Promise<State> {
  const next = { ...(await state()), ...change };
  await ext.storage.local.set({ state: next });
  return next;
}

/** Sends `request` to Velo's host; `undefined` if the host isn't installed or failed. */
async function ask(request: Request): Promise<Answer | undefined> {
  try {
    return (await ext.runtime.sendNativeMessage(HOST, request)) as Answer;
  } catch (error) {
    console.warn("Velo: the native host didn't answer", error);
    return undefined;
  }
}

/** Asks the host for the capture list if it is stale; an unreachable host captures nothing. */
async function refresh(force = false): Promise<State> {
  const current = await state();
  if (!force && Date.now() - current.checkedAt < REFRESH_MS) return current;
  const hello = await ask({ type: "hello" });
  const extensions = isCompatible(hello) ? hello.capture_extensions : [];
  return update({ extensions, checkedAt: Date.now() });
}

/** Shows whether capturing is off on the toolbar button. */
async function showEnabled(enabled: boolean): Promise<void> {
  await ext.action.setBadgeText({ text: enabled ? "" : "off" });
  await ext.action.setTitle({
    title: enabled ? "Velo Download Manager: capturing downloads" : "Velo Download Manager: not capturing",
  });
}

/** Hands `url` to Velo with the cookies the browser has for it; `false` if Velo refused. */
async function handOff(url: string, referer?: string, fileName?: string): Promise<boolean> {
  const cookies = cookieHeader(await ext.cookies.getAll({ url }).catch(() => []));
  const answer = await ask({ type: "download", url, referer, cookies, file_name: fileName });
  const error = failure(answer);
  if (error !== undefined) console.warn(`Velo: couldn't hand over ${url}: ${error}`);
  return error === undefined;
}

/** Downloads Velo gave back to the browser, which mustn't be captured again. */
const returned = new Set<string>();

// Safari has no downloads API, so there its extension only offers the menu and the YouTube
// button.
if (ext.downloads !== undefined) {
  ext.downloads.onCreated.addListener(async (item) => {
    const url = item.finalUrl || item.url;
    if (returned.delete(url) || item.byExtensionId === ext.runtime.id) return;
    const current = await refresh();
    const bypass = Date.now() < current.bypassUntil;
    if (!current.enabled || !shouldCapture({ url, filename: item.filename }, current.extensions, bypass)) {
      return;
    }
    await ext.downloads.cancel(item.id).catch(() => undefined);
    await ext.downloads.erase({ id: item.id }).catch(() => undefined);
    if (!(await handOff(url, item.referrer || undefined, baseName(item.filename)))) {
      // Velo couldn't take it: let the browser download it after all.
      returned.add(url);
      await ext.downloads.download({ url });
    }
  });
}

ext.runtime.onInstalled.addListener(() => {
  ext.contextMenus.create({
    id: "velo-download",
    title: "Download with Velo",
    contexts: ["link", "video", "audio", "image"],
  });
  void refresh(true);
});

ext.runtime.onStartup.addListener(async () => {
  const current = await refresh(true);
  await showEnabled(current.enabled);
});

ext.contextMenus.onClicked.addListener((info) => {
  const url = info.linkUrl ?? info.srcUrl;
  if (url) void handOff(url, info.pageUrl);
});

ext.action.onClicked.addListener(async () => {
  const current = await update({ enabled: !(await state()).enabled });
  await showEnabled(current.enabled);
});

ext.runtime.onMessage.addListener((message: PageRequest, _sender, respond) => {
  switch (message.type) {
    case "bypass":
      void update({ bypassUntil: Date.now() + BYPASS_MS });
      return false;
    case "list_formats":
    case "download_format":
      ask(message).then(respond);
      // The answer comes later.
      return true;
  }
});
