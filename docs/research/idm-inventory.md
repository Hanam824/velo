# IDM feature and UI inventory

This is the reference that Velo's UI spec and feature list are built from. It was collected from internetdownloadmanager.com in October 2026.

Items marked **[U]** were not confirmed on a fetched page; they come from general knowledge of IDM 6.4x. Check them against the screenshots linked at the end before finalising a dialog. Velo copies the layout and workflow only, never IDM's name, logo or icons.

## 1. Features

### Download engine
- **Dynamic segmentation:** see section 3. IDM markets up to "8 times" faster downloads.
- **Connections:** a global "Default max. conn. number". The FAQ suggests 16 or 32 (1–2 on dial-up). There is an "Exceptions:" table on the Connection tab for a per-server count. [U] The dropdown offers 1, 2, 4, 8, 16, 24 and 32.
- **Resume:** works after a lost connection, a shutdown or a power cut, if the server supports it. The dialog shows "Resume capability". Positions are saved several times a minute.
- **Retries:** "Number of retries for each file if downloading failed". With no limit, a failed file goes to the end of the queue and is retried indefinitely (every 30 seconds if it is alone).
- **Option:** "Ignore file modification time changes when resuming a download".
- **Protocols:** HTTP, HTTPS, FTP (option "Use FTP in PASV") and MMS.
- **Proxy / Socks:**
  - "Get System".
  - Automatic configuration script.
  - Manual proxy per protocol.
  - Use the browser's proxy.
  - No-proxy list with `*` wildcards.
  - Proxy login.
- **Authentication:** Basic, Negotiate, NTLM and Kerberos.
  - "Use authorization" in Add URL.
  - Site Logins (URL, user, password; entries can be per directory).
- **User agent:** "User agent for manually added downloads".
- **Download Limits:** a quota such as 40 MB per hour, after which the download pauses and resumes later.
- **Speed Limiter:** global on/off in Downloads→Speed Limiter. [U] There is also a per-download tab: "Use speed limiter", "Maximum download speed … KBytes/sec" and "Remember the speed limit for this server".
- **Temporary directory:** configurable. Segments are joined into one file at the end.
- **Antivirus check:** runs after a download completes, using a program path plus a parameters field with a `[File]` placeholder.

### Browser integration
- **Extension:** the "integration module" for Chrome, Firefox, Edge and Opera.
- **Basic vs Advanced integration:** Advanced uses a network-level driver and catches downloads from any app.
- **File capture:** downloads with an extension in the File Types list are taken over, except on sites in the exception list.
- **Keys:** Ctrl forces the download into IDM; Alt lets the browser keep it.
- **Context menu:** "Download with IDM" and "Download all links with IDM".
- **Drag and drop:** links can be dragged in and finished files dragged out. A floating "drop target" window is available (Tasks→Show drop target).

### Video download panel
- **Settings:** Options→General→"Customize … Download panels in browsers":
  - A checkbox list of media types.
  - "Clear All" (turns the panel off).
  - "Add".
  - "Exceptions" (sites to skip).
  - "Don't start downloading from web-players automatically".
  - "Show download panel for protected content".
- **Behaviour:**
  - The panel appears when a video plays, usually at the top-right corner of the player.
  - Clicking it opens a list of the formats and resolutions the site serves.
  - Choosing one opens Download File Info.
  - The panel needs the app running.
  - Private videos and RTSP aren't supported.
  - Ads get separate entries.
  - Subtitles are offered.
- **Looks:** [U] a small, dark, semi-transparent bar with "Download this video", a "?" and an "×". Menu items look like `MP4 1280x720 (45.3 MB)`, and there is a "Don't show on this site" item.

### Scheduler and queues
- **Built-in queues:** "Main download queue" (yellow file icons) and "Main synchronization queue" (green). Custom queues can be added. A clock overlay means the queue has a schedule.
- **Modes:**
  - "One-time downloading" removes a file from the queue when it completes.
  - "Periodic synchronization" re-checks the server for changed files.
- **Adding files:** the context menu (Add to Queue / Delete from Queue), or drag and drop onto a queue. A "queue selection panel" appears for Download Later.

### Categories
- **Default categories:** Music, Video, Programs and Documents. [U] Also Compressed.
- **Each category has:** a name, a folder, file extensions and optional sites.
- **Editing:** add, edit and delete from the right-click menu.

### Other
- **Site Grabber:** a spider with a 4-step wizard (start page and login; save location; explorer filters; file filters) and a project dialog. Projects can be saved and scheduled.
- **Batch download:** [U] Tasks→Add batch download, using a wildcard URL such as `file*.jpg` with from/to values. There is also a clipboard variant.
- **Clipboard monitoring:** copying a URL with a monitored extension shows a prompt.
- **Command line:** `idman /s` or `idman /d URL [/p path] [/f name] [/q] [/h] [/n] [/a]`.
  - `/s` starts the queue.
  - `/q` exits when done.
  - `/n` is silent.
  - `/a` adds to the queue without starting.
- **Import/export:** of the download list. [U]
- **Sounds:** a sound can be set for each event.
- **Dial-up/VPN:** redial settings.
- **Startup:** autostart with the system.
- **Appearance:** light and dark themes, toolbar skins, and columns and buttons you can customize.

## 2. UI

### Main window
- **Menu bar:** [U] Tasks, File, Downloads, View, Help (plus Registration in IDM).
  - [U] Tasks: Add new download, Add batch download, Add batch download from clipboard, Export, Import, Run site grabber, Show drop target, Exit.
  - Downloads: Pause All, Stop All, Delete All Completed, Start Queue ▸, Stop Queue ▸, Scheduler, Speed Limiter ▸ (Turn on / Turn off / Settings), Options.
- **Toolbar,** in order:
  1. Add URL
  2. Resume
  3. Stop
  4. Stop All
  5. Delete
  6. Delete Completed
  7. Options
  8. Scheduler
  9. Start Queue ▾
  10. Stop Queue ▾
  11. [U] Grabber
  12. Tell a Friend (Velo: Share)
- **Toolbar behaviour:**
  - Buttons are enabled based on the selection.
  - "Delete" asks for confirmation when the download is unfinished.
  - The toolbar has styles: large or small, 3D or Neon.
  - "Customize…" lets you add, remove and reorder buttons.
  - [U] By default the toolbar uses large icons with a label underneath.
- **Left pane:** a closable "Categories" tree. [U] Its layout:
  ```
  All Downloads
    Compressed / Documents / Music / Programs / Video
  Unfinished
    (same categories)
  Finished
    (same categories)
  Grabber projects
  Queues
    Main download queue / Synchronization queue / custom queues
  ```
- **Download list:** a details table. You can sort by clicking a header, and set which columns show, their width and their order by dragging or with right-click→Columns.
  - [U] Columns: **File Name**, **Q**, **Size**, **Status** (percentage, "Complete" or "Error"), **Time left**, **Transfer rate**, **Last Try Date**, **Description**.
  - Each row has a file-type icon. Unfinished rows get a small down-arrow overlay.
- **Row context menu:**
  - Main actions, Add to Queue, Delete from Queue, Properties.
  - [U] Also Open, Open With, Open folder, Move/Rename, Redownload, Refresh download address.
- **Tray:** [U] closing the window minimizes it to the tray. The tray menu has Open, Speed Limiter and Exit.

### Add URL
- A URL box with history.
- "Use authorization" with Login and Password fields.

### Download File Info
- Shown when the app captures a download or after Add URL.
- [U] Fields:
  - URL (combo)
  - Category dropdown with a "+" button
  - Save As path with a "…" button
  - "Remember this path for '<category>' category"
  - Description
  - File icon and size at the top right
- **Buttons:** Start Download, Download Later, Cancel.
- **Option:** "Start download immediately while showing Download File Info dialog".

### Download progress
- **Confirmed:**
  - Progress, average speed, time remaining, state, resume support.
  - A per-connection chart.
  - Pause and Cancel buttons.
  - Closing the dialog stops the download.
  - The start view can be set to normal, minimized or hidden.
- **Segment bar colours:** pink lines mark where segments start, blue is downloaded data, white is what remains.
- [U] **Title:** `NN% <filename>`.
- [U] **Tabs:** Download status | Speed Limiter | Options on completion.
- [U] **Download status tab:**
  - URL.
  - Rows: Status ("Receiving data..."), File size, Downloaded (`x MB (yy.yy %)`), Transfer rate, Time left, Resume capability (Yes/No).
  - A progress bar.
  - Buttons: `<< Hide details` / `Show details >>`, Pause, Cancel.
- [U] **Details section:** the label "Start positions and download progress by connections", then the segment bar, then a table:
  - **N.**
  - **Downloaded**
  - **Info**: Connecting..., Send GET..., Receiving data..., Connection is closed.
- [U] **Options on completion tab:**
  - Show download complete dialog
  - Hang up modem when done
  - Exit when done
  - Turn off computer when done
  - Force processes to terminate

### Download complete
- [U] Shows Address, File size and "The file saved as".
- **Buttons:** Open, Open with..., Open folder, Close.
- **Checkbox:** "Don't show this dialog again", which can be turned back on in Options→Downloads.
- **File icon:** in the bottom-right corner; it can be dragged.

### Properties
- **Fields:** file name, location, URL, description and login, plus an Open button. [U] Also type, status, size, referer and last try date.

### Options tabs
| Tab | Contents |
|---|---|
| General | Startup, Basic/Advanced integration, browser list, clipboard monitoring, Keys, video panel settings |
| File types | Captured extensions; "Don't start downloading automatically from the following sites" |
| Save To | Folder per category, "Change folder for … category on last selected", temporary directory |
| Downloads | Download complete dialog, progress dialog start view, start dialog, "start immediately", queue selection panel, ignore modification time, user agent, virus checking |
| Connection | Default max. connections, per-server exceptions, Download Limits, [U] connection type/speed |
| Proxy / Socks | See *Proxy / Socks* in section 1 |
| Site Logins | URL / login / password list |
| Dial Up / VPN | Connection, credentials, redial count/interval (Velo: skip) |
| Sounds | Event checkboxes, Browse, Play |
| [U] Appearance | Theme, toolbar skin |

### Scheduler
- **Left side:** a list of queues.
- **Right side:** two tabs, **Schedule** and **Files in the queue**.
- **Bottom buttons:** Start Now, Stop, Apply, Close. [U] Also New queue and Delete.
- **Schedule tab:**
  - One-time downloading or Periodic synchronization.
  - Start download at (time). [U] Once at a date, or Daily with Mon–Sun checkboxes.
  - Repeat every N hours and minutes (for sync).
  - Stop download at.
  - Retries.
  - Open file when done.
  - Exit when done.
  - Turn off computer when done.
  - Force processes to terminate.
  - Download Limits.
- **Files in the queue tab:**
  - A list with up and down buttons and drag to reorder.
  - Delete.
  - "Download [N] files at the same time".

### Colours and icons
- **Classic look:** light grey, native-style controls. IDM also has a dark theme.
- **Queue icons:** yellow for download queues, green for sync queues.
- **Velo:** uses its own icon set and colours, with the same meanings (for example, blue for downloaded data in the segment bar).

## 3. Dynamic segmentation (from /support/segmentation.html)
1. A download starts with one connection. The segments are not fixed at the start.
2. When a connection becomes free, IDM "finds the largest segment to download and divide[s] it in half". The free connection starts at the midpoint, and the original connection's end moves to the midpoint.
3. Connections are reused, which skips extra connect and login steps. Fast connections end up taking work from slow ones.
4. Splitting stops when a segment is too small.
5. File positions are saved several times a minute. After a resume, start positions are assigned again.
6. When all segments are done, the parts are assembled into one file. They are kept in the temporary directory until then.

## 4. Browser integration (partly inferred)
- **Documented:**
  - The extension has to be enabled, and the app has to be running for the video panel.
  - Advanced integration uses a network driver.
  - Third-party extensions may not call IDM's command line.
- **Inferred:**
  - The extension talks to the app through Chrome Native Messaging (a host manifest per browser that points to a helper exe, which relays to the running app).
  - It captures downloads with `chrome.downloads` events and response headers, cancels the browser's download, and sends the URL, cookies, referrer, user agent and POST data.
  - Video detection watches media requests and manifests for each tab, and injects an overlay at the top-right of the `<video>` element.

## Sources
- https://www.internetdownloadmanager.com/features2.html
- https://www.internetdownloadmanager.com/support/segmentation.html
- https://www.internetdownloadmanager.com/support/using_idm/using_idm.html
- https://www.internetdownloadmanager.com/support/using_idm/options.html
- https://www.internetdownloadmanager.com/support/using_idm/completeD.html
- https://www.internetdownloadmanager.com/support/using_idm/properties.html
- https://www.internetdownloadmanager.com/support/idm-scheduler/idm_scheduler.html
- https://www.internetdownloadmanager.com/support/idm-scheduler/idm_queues.html
- https://www.internetdownloadmanager.com/support/idm-grabber/idm_grabber.html
- https://www.internetdownloadmanager.com/register/new_faq/video4.html
- https://mirror5.internetdownloadmanager.com/support/comand_line.html
- Screenshots to check [U] items against:
  - https://www.internetdownloadmanager.com/images/idm_screenshot_6_35.png
  - https://www.internetdownloadmanager.com/images/dynamic.jpg
  - https://www.internetdownloadmanager.com/images/queue.png
  - https://www.internetdownloadmanager.com/images/categories.gif
  - https://www.internetdownloadmanager.com/images/idm_download_video_animation-4.gif
