// Which browser downloads Velo takes over.

/** The lowercase extension of a file name or an address's last path segment, without the dot. */
export function fileExtension(nameOrUrl: string): string | undefined {
  let path = nameOrUrl;
  try {
    path = new URL(nameOrUrl).pathname;
  } catch {
    // A plain file name or path.
  }
  const name = path.split(/[\\/]/).pop() ?? "";
  const dot = name.lastIndexOf(".");
  if (dot <= 0 || dot === name.length - 1) return undefined;
  return name.slice(dot + 1).toLowerCase();
}

/** What the browser says about a download it is starting. */
export interface Started {
  url: string;
  /** The full path the browser chose, or empty if it hasn't yet. */
  filename: string;
}

/**
 * Whether Velo should take over a download: an http(s) address, a file type on the capture
 * list (from the file name, else the address), and the user not holding Alt to skip Velo.
 */
export function shouldCapture(item: Started, extensions: readonly string[], bypass: boolean): boolean {
  if (bypass || !/^https?:\/\//i.test(item.url)) return false;
  const extension = fileExtension(item.filename) ?? fileExtension(item.url);
  return extension !== undefined && extensions.includes(extension);
}

/** A `Cookie` header from the browser's cookies for an address, or `undefined` if none. */
export function cookieHeader(cookies: readonly { name: string; value: string }[]): string | undefined {
  if (cookies.length === 0) return undefined;
  return cookies.map((c) => `${c.name}=${c.value}`).join("; ");
}

/** The file name at the end of a path the browser chose, or `undefined` if there is none. */
export function baseName(path: string): string | undefined {
  const name = path.split(/[\\/]/).pop();
  return name ? name : undefined;
}
