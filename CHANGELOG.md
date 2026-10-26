# Changelog

What each release of Velo Download Manager added. Nightly builds between releases are listed on the [releases page](https://github.com/Hanam824/velo/releases).

## v0.2.0

YouTube downloads and browser extensions.

### YouTube

- Paste a YouTube address (watch, shorts, embed, live or youtu.be) into Add URL to see the video's formats.
  - The list shows video with sound, by resolution, then sound only.
  - The best choice is selected.
- High resolutions come as separate video and sound streams. Velo downloads both and joins them into one MP4 without re-encoding.
  - Joining is pure Rust.
  - If the streams can't be joined that way, Velo uses an `ffmpeg` on `PATH` when there is one.
- Each start asks YouTube again for the stream addresses, since they expire, so YouTube downloads resume like any other.
  - Requests are capped at 10 MiB, which YouTube serves at full speed.
- Velo asks YouTube as several apps in turn and uses the first that answers.
  - The web player's signature and n functions run in an embedded JavaScript engine (Boa).
  - These follow the player's classic layout. YouTube's current, obfuscated player isn't handled yet (see the plan's backlog). Until then, Velo relies on the apps that get direct addresses.
- `velo-cli -d <youtube address> [--itag N]` downloads videos too.

### Browser extensions

- An extension for Chrome, Edge, Brave and other Chromium browsers, Firefox (128+) and Safari (16.4+). It talks to the app through the new `velo-nmh` helper, which the app registers with the browsers at every start.
  - **Download capture** (Chromium and Firefox): downloads of the file types on Velo's capture list open in Velo with their page and cookies. Hold Alt to let the browser have one. The toolbar button turns capturing off.
  - **"Download with Velo"** in the context menu of links, images, video and audio.
  - **On YouTube**, a Download button on the player lists the video's formats and hands the chosen one to Velo.
- The extensions aren't in the stores yet.
  - Chromium and Firefox: load them from the release's zips.
  - Safari: use "Velo Safari Extension.app" with Develop → Allow Unsigned Extensions.

### Also

- Duplicate sound streams (YouTube's dynamic-range-compressed copies) are no longer listed.
- A nightly check against YouTube opens an issue when extraction breaks.

## v0.1.0

The first release: a segmented download manager for Windows, macOS and Linux.

### Downloads

- Downloads each file over up to 32 connections. When a connection is idle, the largest remaining range is split in half.
- Saves each range in its own part file, then merges them, checks the size and moves the result into place. A crash never leaves a cut-off file under the real name.
- Pauses and resumes, also after a restart. A file that changed on the server (`ETag` / `Last-Modified`) starts over.
- Retries with backoff.
- Supports Basic authentication, referer and cookies, a custom user agent, and system or manual proxies (HTTP, HTTPS, SOCKS5).
- Has a total speed limit, a per-download limit, and connection counts per server.

### The app

- **Main window:**
  - A menu bar and a toolbar.
  - A Categories tree that filters by category and by Unfinished / Finished, and lists the queues.
  - A sortable download list with file-type icons and a context menu.
  - A status bar.
- **Dialogs:**
  - Add URL, then Download File Info, which detects the category and remembers folders per category.
  - The progress dialog, with the segment bar, the connections, a speed limit and options for when the download completes.
  - Download complete and Properties.
- **Options:** General (clipboard monitoring), Save To, Downloads, Connection, Proxy and Appearance (light, dark or system).
- **Categories:** add, edit or delete them.
- **Queues and the scheduler:**
  - Start and stop times on chosen days.
  - Downloads at a time.
  - Retries.
  - Exit or shut down when done.
- **Adding many downloads:**
  - Clipboard monitoring for chosen file types.
  - Batch downloads from an address pattern.
  - Import and export of the list as plain text.
- **Desktop integration:**
  - A tray icon, with closing to the tray.
  - A single instance.
  - Notifications.
  - Open file / open folder.

### Command line

- `velo-cli` adds downloads to the running app (`-d`, `-p`, `-f`, `-n`, `-a`, `-s`), or downloads on its own with `-q` / `--headless`.

### Packaging

- Installers for every platform: a per-user NSIS installer for Windows (x64, ARM64), a `.dmg` for macOS 15+ (Apple Silicon, Intel), and `.deb` and AppImage for Linux x86_64. Plain archives are still published.
- Not signed or notarized yet.
