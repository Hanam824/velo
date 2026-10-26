// The messages between the extension and Velo's native-messaging host (velo-nmh). See
// docs/native-messaging.md for the protocol.

/** The name the browser knows Velo's host by. */
export const HOST = "io.github.hanam824.velo";

/** The protocol version this extension speaks. */
export const PROTOCOL = 1;

/** The host's answer to `hello`. */
export interface Hello {
  type: "hello";
  protocol: number;
  app_version: string;
  /** File extensions to capture, lowercase without the dot. */
  capture_extensions: string[];
}

/** One way to download a video: its label and the streams (itags) it needs. */
export interface Choice {
  label: string;
  itags: number[];
}

/** The host's answer to `list_formats`. */
export interface Formats {
  type: "formats";
  title: string;
  author: string;
  choices: Choice[];
}

/** The host's answer to `download` and `download_format`. */
export interface Outcome {
  type: "result";
  ok: boolean;
  error?: string;
}

/** The host's answer to a request it couldn't carry out. */
export interface Failure {
  type: "error";
  error: string;
}

/** A request to the host. */
export type Request =
  | { type: "hello" }
  | { type: "download"; url: string; referer?: string; cookies?: string; file_name?: string }
  | { type: "list_formats"; url: string }
  | { type: "download_format"; url: string; itags: number[] };

/** Any answer from the host. */
export type Answer = Hello | Formats | Outcome | Failure;

/** Whether a `hello` answer comes from a host this extension can work with. */
export function isCompatible(answer: unknown): answer is Hello {
  const hello = answer as Partial<Hello> | null;
  return (
    hello?.type === "hello" &&
    hello.protocol === PROTOCOL &&
    Array.isArray(hello.capture_extensions)
  );
}

/** The error message in an answer, or `undefined` if it reports success. */
export function failure(answer: Answer | undefined): string | undefined {
  if (answer === undefined) return "Velo didn't answer";
  if (answer.type === "error") return answer.error;
  if (answer.type === "result" && !answer.ok) return answer.error ?? "Velo refused";
  return undefined;
}

/** A request from the extension's own pages (the YouTube button) to its background. */
export type PageRequest =
  | { type: "list_formats"; url: string }
  | { type: "download_format"; url: string; itags: number[] }
  /** The user is holding Alt: let the next download go to the browser. */
  | { type: "bypass" };
