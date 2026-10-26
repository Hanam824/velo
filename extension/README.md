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

The manifest's `key` fixes the extension ID to `knncpmilkobjkfcjafpleodokkhcaing`, the one Velo's
host manifest allows. The matching private key isn't kept: a store gives a published extension
its own key.
