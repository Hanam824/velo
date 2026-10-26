// Finding the video ID in a YouTube address, as velo-extract does.

const HOSTS = new Set([
  "youtube.com",
  "www.youtube.com",
  "m.youtube.com",
  "music.youtube.com",
  "www.youtube-nocookie.com",
]);

/** Whether `id` has the shape of a video ID: 11 characters of A–Z a–z 0–9 - _. */
export function isVideoId(id: string): boolean {
  return /^[A-Za-z0-9_-]{11}$/.test(id);
}

/** The video ID in a YouTube watch, shorts, embed, live or youtu.be address. */
export function videoId(address: string): string | undefined {
  let url: URL;
  try {
    url = new URL(address);
  } catch {
    return undefined;
  }
  if (url.protocol !== "https:" && url.protocol !== "http:") return undefined;
  const host = url.hostname.toLowerCase();
  const segments = url.pathname.split("/").filter((s) => s !== "");
  let id: string | undefined;
  if (host === "youtu.be") {
    id = segments[0];
  } else if (HOSTS.has(host)) {
    if (segments[0] === "watch") id = url.searchParams.get("v") ?? undefined;
    else if (["shorts", "embed", "live", "v"].includes(segments[0] ?? "")) id = segments[1];
  }
  return id !== undefined && isVideoId(id) ? id : undefined;
}
