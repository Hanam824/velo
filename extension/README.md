# Velo browser extension

Hands downloads and YouTube videos to Velo Download Manager through its native-messaging host
(`velo-nmh`, see [`docs/native-messaging.md`](../docs/native-messaging.md)). One TypeScript code
base, bundled with esbuild for each browser in `manifests/`.

- **Download capture:** a download of a file type on Velo's capture list (Options → General) is
  cancelled in the browser and opened in Velo with its page and cookies. Hold Alt while clicking to
  let the browser have it. The toolbar button turns capturing off and on ("off" on the badge).
  If Velo can't take a download, the browser downloads it after all.
- **Context menu:** "Download with Velo" on links, images, video and audio.
- **YouTube:** a **Download** button at the top right of the player lists the video's formats
  (asked of the host, best first) and hands the chosen one to Velo.

## Develop

```sh
cd extension
npm ci
npm run check   # TypeScript
npm test        # unit tests of the pure modules (node --test)
npm run build   # dist/<browser>/
```

Needs Node 22. `src/protocol.ts`, `src/capture.ts` and `src/youtube-id.ts` are pure and
tested; `src/background.ts` (service worker), `src/youtube.ts` and `src/bypass.ts` (content
scripts) use the browser APIs.

## Try it in Chrome, Edge or Brave

1. Install Velo (it registers `velo-nmh` with the browsers when it starts) and run it once.
2. Open `chrome://extensions`, turn on Developer mode, choose **Load unpacked** and pick
   `extension/dist/chromium`.

## Try it in Firefox

1. Install Velo and run it once (it registers `velo-nmh` for Firefox when Firefox has a profile).
2. Open `about:debugging#/runtime/this-firefox`, choose **Load Temporary Add-on…** and pick
   `extension/dist/firefox/manifest.json`. A temporary add-on lasts until Firefox restarts; a
   permanent install needs the add-on signed by Mozilla (in the backlog).

Firefox needs version 128 or later (the ESR). Its manifest uses an event page
(`background.scripts`) instead of a service worker and names the add-on `velo@hanam824.github.io`,
the ID Velo's host manifest allows.

## Safari

Safari runs extensions from inside a macOS app: `scripts/build-safari.sh` (needs Xcode) builds
"Velo Safari Extension.app" from `dist/safari` with Apple's converter, puts in Velo's native
handler (`safari/SafariWebExtensionHandler.swift`) and its sandbox exception
(`safari/Extension.entitlements`), and zips it. Releases attach it as
`velo-<version>-safari-extension-macos.zip`.

The Safari extension can't start `velo-nmh`, so its handler sends each native message to the
Velo app over the app's socket (`/tmp/velo-<hash>.sock`, the same name the app computes), and the
app answers it as `velo-nmh` would; if the app isn't running, the handler starts it. Safari has
no downloads API, so there is no download capture in Safari: only "Download with Velo" in the
context menu and the YouTube button.

The app is signed ad hoc, not with a Developer ID, so Safari runs it only after:

1. Unzip it and move "Velo Safari Extension.app" to Applications, then open it once.
2. In Safari, Settings → Advanced: turn on **Show features for web developers**.
3. Develop → **Allow Unsigned Extensions** (asked again after each Safari restart).
4. Settings → Extensions: turn on **Velo Download Manager** and allow it on the sites you want.

## Extension IDs

The Chromium manifest's `key` fixes the extension ID to `knncpmilkobjkfcjafpleodokkhcaing`, the one Velo's
host manifest allows. The matching private key isn't kept: a store gives a published extension
its own key.
