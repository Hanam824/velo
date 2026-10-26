# Native messaging

Velo's browser extension talks to the `velo-nmh` program (the native-messaging host), which the
browser starts when the extension connects. `velo-nmh` ships next to the app (`velo`), and the app
registers it with the browsers at every start (`velo_platform::register_native_host`), under the
name `io.github.hanam824.velo`:

| | Chromium browsers | Firefox |
|---|---|---|
| macOS | `~/Library/Application Support/<browser>/NativeMessagingHosts/` | `~/Library/Application Support/Mozilla/NativeMessagingHosts/` |
| Linux | `~/.config/<browser>/NativeMessagingHosts/` | `~/.mozilla/native-messaging-hosts/` |
| Windows | `HKCU\Software\<vendor>\<browser>\NativeMessagingHosts\…` → manifest in Velo's data folder | `HKCU\Software\Mozilla\NativeMessagingHosts\…` |

Only browsers whose profile folder exists get a manifest on macOS and Linux. The manifests
allow the extension IDs `knncpmilkobjkfcjafpleodokkhcaing` (Chromium; fixed by the public key in
the extension's manifest) and `velo@hanam824.github.io` (Firefox).

Safari is different: its extension's native side is a handler inside "Velo Safari Extension.app"
(`extension/safari/SafariWebExtensionHandler.swift`), sandboxed and unable to start `velo-nmh`.
It sends each request to the Velo app as an IPC `native` message on the app's socket, and the app
answers it with the same code (`velo_nmh::answer`), starting the app first if needed.

## Messages

Each message is JSON preceded by its length as a 32-bit number in the machine's byte order, at
most 1 MB. The extension sends one request at a time and gets one answer. The protocol version
is `1`; an incompatible change raises it, and the extension checks it with `hello`. New request
types and new optional fields are added without raising it.

| Request | Answer |
|---|---|
| `{"type": "hello"}` | `{"type": "hello", "protocol": 1, "app_version": "0.1.0", "capture_extensions": ["zip", …]}` |
| `{"type": "download", "url", "referer"?, "cookies"?, "file_name"?}` | `{"type": "result", "ok": true}` or `{"type": "result", "ok": false, "error": "…"}` |
| `{"type": "list_formats", "url"}` | `{"type": "formats", "title", "author", "choices": [{"label", "itags": [137, 140]}, …]}` or `{"type": "error", "error": "…"}` |
| `{"type": "download_format", "url", "itags": [137, 140]}` | `result` |

- `download` opens Download File Info in the app, as for a link added by hand; the referer and
  cookies are sent with the download's requests and kept in memory only.
- `list_formats` is answered by the host itself (with `velo-extract`), so the format menu works
  while the app starts. The choices are best first.
- `download_format` adds the chosen streams without asking and starts them; two itags are a
  video and its sound, joined when both are in.
- If the app isn't running, the host starts it (detached, so it survives the browser closing the
  host) and waits up to 10 seconds for it.
- Anything else gets `{"type": "error", "error": "…"}`; the host keeps running.
