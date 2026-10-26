# Changelog

What each release of Velo Download Manager added. Nightly builds between releases are listed on the [releases page](https://github.com/Hanam824/velo/releases).

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
