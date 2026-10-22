# Pirosman Studio

**Pirosman Studio** is an independently developed, open-source video editor built with Rust and Tauri. It combines a professional editing workflow with Git-like version control and real-time (Live) collaboration.

Its workflows and features are informed by the study of professional non-linear editing (NLE) practice, including DaVinci Resolve's public documentation. Pirosman Studio contains no code from DaVinci Resolve or any other closed-source program, is not a copy of it, and does not claim feature parity with it. The project is not affiliated with Blackmagic Design.

> **Status:** early alpha (`0.1.0-alpha`). Expect bugs and changes to the project format.

## Project goals

When editing video, managing the history of changes, restoring earlier versions and combining the work of several people is often difficult and inefficient. Professional editors can also be expensive or closed source.

Pirosman Studio addresses this with two complementary mechanisms:

- **Git-like version control:** keeps the editing history and lets you create branches, compare changes, restore an earlier state and merge changes made independently.
- **Live Sync:** lets several people work on the same project at the same time and see each other's changes in real time.

The goal is to bring video editing, version control and team collaboration together in one environment.

## Installation and launch

Pirosman Studio runs on 64-bit Windows 10 (version 1809 or later) and Windows 11.

### 1. Setup installer

The recommended download is `Pirosman-Studio_..._x64-setup.exe`.

1. Run the setup installer (available from the latest Release).
2. Follow the installation steps.
3. When installation finishes, start Pirosman Studio.

The installer installs into your user profile and does not need administrator rights. If needed, it also tries to install the Microsoft Edge WebView2 Runtime, which requires an internet connection to download. The installer also registers an uninstaller.

If a Windows security warning appears, use `More info → Run anyway` only if you are sure the installer came from a trusted source.

### 2. Portable version

If you use `Pirosman-Studio_..._x64-portable.zip`:

1. Extract the ZIP archive to a folder of your choice.
2. Keep the extracted folder together: `Pirosman Studio.exe`, `ffmpeg\` and `ofx\` must stay in the same folder.
3. Run `Pirosman Studio.exe`.

The portable version needs no installation. The `README.txt` inside the ZIP (source: [`packaging/PORTABLE_README.txt`](packaging/PORTABLE_README.txt)) covers troubleshooting: the SmartScreen warning, the WebView2 Runtime, a damaged FFmpeg, sharing projects and Live Sync.

### 3. Media processing

Pirosman Studio uses FFmpeg and FFprobe to read media information, preview video and export.

**FFmpeg 9.0.2 and FFprobe are bundled with the app.** The version is pinned and its checksum is verified at build time. You do not need to install WinGet, run terminal commands or download FFmpeg separately. FFmpeg is distributed under its own GPL v3 license (see [License](#license)).

## Projects and files

### The `.pirosman` project format

Pirosman Studio uses its own project format:

- **Versioned, structured JSON:** project files are human-readable.
- **Backward compatibility:** older projects can be opened.
- **Unknown fields are preserved:** fields from a newer format should not be lost when a project is opened in an older version.
- **Safe saving:** projects are saved atomically, and the previous version is kept in a `.bak` file.
- **Save status:** the title bar shows `Saved` or `● Unsaved` and the project file's path.
- **Rename projects:** click the project name to rename it without leaving the app.
- **Recent projects:** the app remembers the last 12 projects, and the `File` menu shows the last 8. Missing files are greyed out.
- **Reload from disk:** re-reads the saved project and its history.
- **Unsaved-changes protection:** before you create a new project or open another one, the app warns you about losing unsaved changes.
- **Drag and drop:** drop a `.pirosman` file on the window to open it, or drop media files to import them.

**Limitations:** autosave is not available yet. `.pirosman` files are not registered with Windows, so double-clicking one does not open the app.

### Media management

- **Media stays where it is:** files are not copied into the project automatically, and a SHA-256 hash identifies each file.
- **Offline detection:** moved, changed or missing files are detected as offline media.
- **Safe relinking:** relinking checks the content hash so the wrong file is not attached to the project by mistake.
- **Background processing:** proxy files, thumbnails and audio waveforms are generated automatically in the background.
- **Metadata panel:** codecs, streams, frame rate, SHA-256 hash and how many clips use each file.
- **Short media codes:** each file gets an identifier such as `M01`, `M02` and so on.

## Collaboration and sharing projects

### Git-like version control

Pirosman Studio has its own Git-like version control system. It is **not a direct Git integration** and does not require Git to be installed.

- **Revision history:** a history of changes with a branch graph, author, time and clip count.
- **Branches:** create, switch between and delete branches. Unsaved work can be stashed before switching.
- **Restore a revision:** restore an earlier version in a single Undo step.
- **Readable changes:** for example, `intro.mp4 trimmed 14 frames`.
- **Three-way merge:** merge changes and resolve conflicts with `Keep Ours` and `Keep Theirs`.
- **Visual change marks:** clips changed since the last revision get a yellow, pencil-style mark. The number of changes appears on the branch indicator and the page tab.
- **Quick commit:** `Ctrl+Shift+C` from any page. The Inspector shows a `Since last revision` list.
- **History integrity checks:** content-addressed history is used to detect tampering with history files.
- **`.pirosman-bundle` exchange format:** export branches and history, optionally with media. Imported work is stored in `incoming/<branch>`, and media is relinked automatically by hash.

### Live Sync

Live Sync lets several people work on the same project at the same time over a local network.

- **Revisions | Live:** the switch in the top-right corner toggles between the history and Live Sync modes. The normal Revisions mode does not use the network.
- **Host a session:** the host sees the session code and network addresses, and can copy an invitation or open a window for a second participant.
- **Join a session:** a participant enters the network address and the session code.
- **Cursors and playheads:** participants see each other's named cursors, playheads on the timeline and coloured outlines on selected clips in real time.
- **Participant status:** initials appear in the title bar, and the app shows notifications when participants join or leave.
- **Change sync:** edits are sent immediately, and each remote edit is recorded as a single step in the Undo history.

**Security limitations:** Live Sync is meant for a local network and uses an unencrypted `ws://` connection. When changes conflict, the last write wins.

### Sharing a project

To share a project's history and media files:

1. Open `Revisions → Exchange → Export bundle`.
2. Tick `Include media` if you also want to include the media files in the bundle.
3. Send the resulting `.pirosman-bundle` file to the other person.
4. The recipient imports the bundle in the app.

For Live Sync, both computers must be on the same local network. If needed, allow the app network access in Windows Firewall.

## Key features

### Editing

- Multitrack timeline;
- Add, move, split, trim and delete clips;
- Insert and Overwrite edits;
- Undo/Redo with descriptive action names;
- Transitions, text titles and markers;
- Timeline zoom, selection and snapping;
- Constant speed changes and reverse playback;
- OpenFX (OFX) plugin support; effects are computed at render time (see [Known limitations](#known-limitations)).

### Image and visual effects

- Scale, position and rotation controls;
- Crop;
- Gaussian Blur, Glow, Vignette and Film Grain;
- Chromatic Aberration;
- `.cube` LUTs;
- Single or dual monitor layouts and floating monitors.

### Color correction

- Lift, Gamma and Gain;
- Contrast and saturation;
- Waveform and RGB Parade scopes;
- Save a frame as PNG (`Grab Still`).

### Audio

- Audio track mixer;
- Level and pan;
- Three-band equalizer;
- Mute and Solo;
- Audio meters and automatic waveform generation.

### Export and rendering

- H.264, HEVC, MPEG-4, ProRes and DNxHR video formats;
- WAV and M4A audio formats;
- Cancellable export with progress display;
- Preview the FFmpeg command and filter graph before rendering;
- Technical verification of the exported file with FFprobe;
- Preview the render result inside the app.

### Project interoperability

- Import FCP 7 XML and OpenTimelineIO;
- Export OpenTimelineIO;
- Limited import of DaVinci Resolve `.drp` and `.drt` files that embed supported FCP7 XML or OTIO data;
- Share a project's history and media in a single bundle.

## Interface and workflow

- **Five pages:** Media, Edit, Color, Revisions and Deliver, reachable from the bottom navigation bar or with `Shift+1`–`Shift+5`.
- **Keyboard shortcuts:** about 50 shortcuts and mouse actions. Press `?` to see the full list. Single-key shortcuts do not work while you are typing in a text field.
- **Windows context menus:** right-click menus, including Cut, Copy and Paste in text fields.
- **Menu bar:** File, Edit, Timeline, Revisions and Help.
- **Named Undo/Redo:** for example `Undo Trim clip`, with up to 200 steps of history.
- **Flexible layout:** resizable panels that remember their sizes; swappable monitors, single or dual monitor layouts and floating monitors.
- **Inspector/Mixer toggle:** quickly show parameters or the audio mixer.
- **In-app dialogs and notifications** instead of standard browser pop-ups.
- **Pirosmani Midnight theme:** with the Pirosmani logo in the title bar, the About window and the app icon.
- **Accessibility:** respects the Windows reduced-motion setting and shows visible keyboard focus outlines.

## Diagnostics and interoperability tools

- **Help → Diagnostics & feature status:** FFmpeg paths, versions and encoders, the feature status registry, OpenFX plugin discovery, and the cache and history folders.
- **Help → FFmpeg status:** checks the state of FFmpeg and its available components.
- **Help → Resolve binary & interop inspector:** a read-only inspection of DaVinci Resolve binaries and interoperability information.

The Resolve inspector reads only the list of installed files, PE headers, version information and export names. It does not decompile or disassemble Resolve code, does not load or run it, and copies nothing.

## OpenFX effects

- **Effects library:** on the Edit page, next to the Media Pool. Built-in effects and every installed OpenFX plugin, with a separate tab for each vendor (for example Sapphire, BCC).
- **Usage:** drag an effect onto a clip on the timeline, or double-click it to add it to the selected clips. Parameters are edited in the Inspector.
- **Safety:** plugins run in a separate process (`pirosman-ofxhost.exe`), so a faulty plugin cannot crash the editor.
- **Rendering:** export, Render preview and the paused monitor frame are computed exactly through the plugin.
- **Sample plugins:** the app ships with "Pirosman Samples" (Invert, Tint, Posterize, Echo).

## Technical foundation

- **Rust:** project data, editing logic, revisions, diffing and merging.
- **Tauri 2:** the desktop application shell.
- **React, TypeScript and Zustand:** the user interface and state management.
- **FFmpeg and FFprobe:** media processing, rendering, export and file validation.

## Testing and reliability

- **More than 100 automated tests**, including real FFmpeg renders whose output is checked frame by frame.
- **Start-up checks:** the app shows a clear message if WebView2 is missing or FFmpeg is damaged.
- **Render control:** progress display, cancellation and technical verification of the exported file.

Test counts and supported features apply to the specific build.

## Known limitations

- Live video preview approximates some effects; the final result is produced at render time.
- OpenFX parameters are constant per clip (no keyframes); overlay/interact, transition and generator plugins and GPU rendering are not supported. Commercial plugins (Sapphire, BCC) require their own licenses.
- Node-based compositing, keyframes, speed ramps and some professional color and audio features are not available yet.
- Autosave is not available.
- `.pirosman` files are not associated with Windows.
- Live Sync is meant for a local network, uses an unencrypted connection, and resolves conflicts by letting the last change win.
- Media paths in a project are absolute; when moving to another computer, use a project bundle and media relinking.
- `.drp`/`.drt` import is limited to files that embed supported FCP7 XML or OTIO data.
- macOS and Linux are not currently supported.

## Building from source (for developers)

This section is for developers only; users of the installer or the portable version do not need these tools.

| Folder | Contents |
|---|---|
| `src/` | User interface (React, TypeScript) |
| `src-tauri/` | Desktop app shell: Tauri commands, configuration, icons, installer hooks |
| `crates/pirosman-core/` | Project model, editing, rendering, revisions; `src/parity.json` is the feature status registry built into the app |
| `crates/pirosman-ofxhost/` | Out-of-process host for OpenFX plugins |
| `crates/pirosman-ofx-samples/` | Sample OpenFX plugins |
| `scripts/` | PowerShell build and packaging scripts |
| `packaging/` | Text files included in releases: the portable README and the FFmpeg notice |

Requirements: Windows 10/11 x64, Rust (stable, MSVC toolchain), Node.js and npm, plus Visual Studio or Visual Studio Build Tools with the "Desktop development with C++" workload and a Windows 10/11 SDK (required by Rust's MSVC linker). The first build needs an internet connection to download the pinned FFmpeg. On Windows, `tauri dev` and `tauri build` run `npm run fetch-ffmpeg` and `npm run build:ofx` automatically.

```bash
npm ci
npm run tauri dev
```

Checks and tests. In a fresh checkout, run `npm run fetch-ffmpeg` and `npm run build:ofx` once before running `cargo`: they prepare the `src-tauri\ffmpeg\` and `src-tauri\ofx\` folders, without which the app's Rust code does not build.

```bash
npm ci
npm run typecheck
npm run fetch-ffmpeg
npm run build:ofx
cargo test --workspace
```

The FFmpeg integration tests are skipped when no FFmpeg is found. To force them to run against the bundled FFmpeg, set `PIROSMAN_TEST_BUNDLED_DIR` (the FFmpeg folder) and `PIROSMAN_TEST_REQUIRE_BUNDLED`; a missing or rejected FFmpeg then fails the tests instead of skipping them. The OpenFX render test additionally needs debug builds of `pirosman-ofxhost` and `pirosman-ofx-samples`. In PowerShell:

```powershell
cargo build -p pirosman-ofxhost -p pirosman-ofx-samples
$env:PIROSMAN_TEST_BUNDLED_DIR = "$PWD\src-tauri\ffmpeg"
$env:PIROSMAN_TEST_REQUIRE_BUNDLED = "1"
cargo test --workspace
```

Building the installer and the portable ZIP (into `dist-release\`):

```bash
powershell -ExecutionPolicy Bypass -File scripts/package-windows.ps1
```

## License

Pirosman Studio is available under your choice of the MIT or Apache-2.0 license: see [`LICENSE-MIT`](LICENSE-MIT) and [`LICENSE-APACHE`](LICENSE-APACHE).

The FFmpeg bundled in the installer and the portable version is distributed under its own GPL v3 license: see [`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md).

## Keyboard shortcuts

**Press `?` in Pirosman Studio to see all keyboard shortcuts.**

Single-key shortcuts do not work while you are typing in a text field.

### Project and files

| Shortcut | Action |
|---|---|
| `Ctrl+N` | New project |
| `Ctrl+O` | Open project |
| `Ctrl+S` | Save |
| `Ctrl+Shift+S` | Save as |
| `Ctrl+I` | Import media |
| `Ctrl+Shift+C` | Commit a revision (the project must be saved first) |
| `Ctrl+Enter` | Commit a revision while typing its description |

### Pages

| Shortcut | Action |
|---|---|
| `Shift+1` | Media |
| `Shift+2` | Edit |
| `Shift+3` | Color |
| `Shift+4` | Revisions |
| `Shift+5` | Deliver |

### Playback and navigation

| Shortcut | Action |
|---|---|
| `Space` | Play / pause |
| `K` | Stop |
| `L` | Play |
| `←` / `→` | Previous / next frame |
| `Shift+←` / `Shift+→` | Jump back / forward 1 second |
| `Home` / `End` | Go to the start / end of the timeline |

### Marking and placing clips

| Shortcut | Action |
|---|---|
| `I` / `O` | Mark In / Out in the active viewer (on the source clip or the timeline render range) |
| `Alt+X` | Clear the timeline In/Out marks |
| `M` | Add a marker at the playhead |
| `F9` | Insert the marked source range at the playhead; later clips move right |
| `F10` | Overwrite with the marked source range at the playhead |

### Editing

| Shortcut | Action |
|---|---|
| `Ctrl+Z` | Undo |
| `Ctrl+Shift+Z` or `Ctrl+Y` | Redo |
| `Ctrl+B` | Cut all tracks at the playhead |
| `Ctrl+C` | Copy selected clips |
| `Ctrl+V` | Paste clips at the playhead |
| `Ctrl+D` | Duplicate selected clips |
| `Ctrl+A` | Select all clips |
| `Delete` / `Backspace` | Delete selected clips |
| `Shift+Delete` | Ripple Delete: delete the clip and close the gap |
| `Esc` | Clear the selection / close a dialog or menu |

### Timeline tools

| Shortcut | Action |
|---|---|
| `A` | Selection tool |
| `B` | Blade tool; press again to return to the selection tool |
| `Y` | Slip tool |
| `U` | Slide tool |
| `R` | Roll tool |
| `N` | Toggle snapping |

### Timeline view

| Shortcut | Action |
|---|---|
| `=` / `+` | Zoom in |
| `-` | Zoom out |
| `Ctrl+mouse wheel` | Zoom around the cursor position |
| `Shift+Z` | Fit the whole timeline to the window |

### Mouse actions

| Action | Result |
|---|---|
| `Ctrl+click` or `Shift+click` on a clip | Add the clip to / remove it from the selection |
| Double-click a clip | Move the playhead to the start of the clip |
| Double-click a slider (mixer, color) | Reset to the default value |
| Right-click | Open the relevant context menu |

### Help

| Shortcut | Action |
|---|---|
| `?` | Show the list of keyboard shortcuts |
