# Icon Manifest

## Native app, window, tray, and installer icons

The durable native source is `src-tauri/icons/voicel-icon-source.svg`. Its Activity path is copied from the installed `lucide-react` 0.468.0 package (`dist/esm/icons/activity.js`, ISC license) and matches the `Activity size={17} strokeWidth={2.2}` mark in `src/App.tsx`. The generated raster master is `src-tauri/icons/voicel-icon-master.png` (512x512 PNG, 45488 bytes, SHA-256 `95EE758839E809F82DDD1A1A8FB75AC3AEEAB0F29306D376243D0AA3A3B08394`).

Generated with the installed Tauri CLI from the durable SVG source:

```text
pnpm tauri icon --output src-tauri/icons src-tauri/icons/voicel-icon-source.svg --verbose
```

Tauri-derived outputs in `src-tauri/icons`:

- Desktop/app/window/tray: `icon.png`, `32x32.png`, `64x64.png`, `128x128.png`, `128x128@2x.png`, `icon.ico`, and `icon.icns`.
- Windows AppX/Store: `StoreLogo.png`, `Square30x30Logo.png`, `Square44x44Logo.png`, `Square71x71Logo.png`, `Square89x89Logo.png`, `Square107x107Logo.png`, `Square142x142Logo.png`, `Square150x150Logo.png`, `Square284x284Logo.png`, and `Square310x310Logo.png`.

`icon.ico` contains 16x16, 24x24, 32x32, 48x48, 64x64, and 256x256 entries. The existing `src-tauri/tauri.conf.json` bundle icon paths remain explicit and unchanged; version metadata is now `0.1.1` in the Tauri config, frontend package, and Rust package so a rebuilt installer/executable gets a new Windows shell identity. The stable identifier remains `com.voicel.desktop`. Tauri uses these generated assets for app/window surfaces and installers; the tray inherits `app.default_window_icon()`. All native icon inputs and outputs are local project files; no remote runtime asset or network dependency is required.

The native composition preserves the in-app identity while adding small-size separation: a dark product plate uses the existing neutral/green palette, the circle uses the signal gradient anchored at `#c9f970`, and the sourced Activity path uses a signal stroke over a restrained dark contrast underlay. No custom waveform or decorative logo path is introduced.

Voicel uses one outline icon family across the interface. Icons are imported from the local `lucide-react` package; no remote runtime asset is required.

| UI area | Icon | Source | Package/file | Size/stroke | Reason |
| --- | --- | --- | --- | --- | --- |
| Desktop/taskbar/installer | Activity in signal circle | Lucide + project-owned composition | lucide-react/Activity -> src-tauri/icons/voicel-icon-source.svg | 512px raster master; signal stroke 2.2 with 3.4 contrast underlay; ICO 16/24/32/48/64/256 | Preserves the live listening mark while retaining plate/ring/path separation at Windows shell sizes |
| Brand mark | Activity | Lucide | lucide-react/Activity | 17px, stroke 2.2 | Signals live listening without a fabricated logo mark |
| Primary navigation | Mic2 | Lucide | lucide-react/Mic2 | 18px, stroke 1.85 | Live dictation view |
| Primary navigation | History | Lucide | lucide-react/History | 18px, stroke 1.85 | Finished transcript view |
| Primary navigation | BookOpen | Lucide | lucide-react/BookOpen | 18px, stroke 1.85 | Custom vocabulary view |
| Primary navigation | Cpu | Lucide | lucide-react/Cpu | 18px, stroke 1.85 | Local model view |
| Primary navigation | SlidersHorizontal | Lucide | lucide-react/SlidersHorizontal | 18px, stroke 1.85 | Settings view |
| Navigation/state | ChevronRight | Lucide | lucide-react/ChevronRight | 15px, stroke 2 | Current view and route affordance |
| Status | CircleCheck | Lucide | lucide-react/CircleCheck | 15-20px, stroke 1.8-2 | Committed, selected, and successful states |
| Status | CircleAlert | Lucide | lucide-react/CircleAlert | 15-24px, stroke 1.8-2 | Recoverable error and availability states |
| Status | Loader2 | Lucide | lucide-react/Loader2 | 14-25px, stroke 1.7-2 | Honest pending/loading state |
| Recording | Volume2 | Lucide | lucide-react/Volume2 | 19px, stroke 1.7 | Signal inspector heading |
| Recording | Square | Lucide | lucide-react/Square | 14px, stroke 2.3 | Stop action |
| Recording | Zap | Lucide | lucide-react/Zap | 14-15px, stroke 1.8 | Keyboard/native behavior note |
| Clipboard actions | Copy | Lucide | lucide-react/Copy | 15-16px, stroke 2 | Copy transcript action |
| Clipboard actions | Clipboard | Lucide | lucide-react/Clipboard | 18px, stroke 1.8 | Text delivery settings |
| Vocabulary actions | Plus | Lucide | lucide-react/Plus | 16px, stroke 2 | Add custom word |
| Vocabulary actions | Save | Lucide | lucide-react/Save | 15-16px, stroke 1.9-2 | Save edited word/settings |
| Destructive actions | Trash2 | Lucide | lucide-react/Trash2 | 15-16px, stroke 1.9 | Clear/delete actions |
| Models | Download | Lucide | lucide-react/Download | 15-18px, stroke 1.8-2 | Install a local model |
| Settings | Settings2 | Lucide | lucide-react/Settings2 | 18px, stroke 1.8 | Startup settings section |
| Settings | Keyboard | Lucide | lucide-react/Keyboard | 15-18px, stroke 1.8 | Hotkey affordance and section |
| Utility | RefreshCw | Lucide | lucide-react/RefreshCw | 15-16px, stroke 2 | Retry/recovery action |
| Utility | X | Lucide | lucide-react/X | 15-16px, stroke 2 | Cancel/dismiss action |
| Utility | Minus | Lucide | lucide-react/Minus | 14px, stroke 2 | Minimize the custom-chrome window |
| Utility | Check | Lucide | lucide-react/Check | 13-18px, stroke 2-2.4 | Compact confirmation |
