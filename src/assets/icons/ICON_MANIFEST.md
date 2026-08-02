# Icon Manifest

## Native app, window, tray, and installer icons

The generated raster identity master is the single source for native Tauri icon outputs. Its durable project copy is `src-tauri/icons/voicel-icon-master.png` (1254x1254 PNG, 940900 bytes, SHA-256 `DB7C99CDFD15AB911DD988AB64A3C1F6A9CF70032EAB46DD42D8AC67F299F407`).

Generated with the installed Tauri CLI from the durable copy:

```text
pnpm tauri icon --output src-tauri/icons src-tauri/icons/voicel-icon-master.png --verbose
```

Tauri-derived outputs in `src-tauri/icons`:

- Desktop/app/window/tray: `icon.png`, `32x32.png`, `64x64.png`, `128x128.png`, `128x128@2x.png`, `icon.ico`, and `icon.icns`.
- Windows AppX/Store: `StoreLogo.png`, `Square30x30Logo.png`, `Square44x44Logo.png`, `Square71x71Logo.png`, `Square89x89Logo.png`, `Square107x107Logo.png`, `Square142x142Logo.png`, `Square150x150Logo.png`, `Square284x284Logo.png`, and `Square310x310Logo.png`.
The existing `src-tauri/tauri.conf.json` bundle icon paths remain unchanged. Tauri uses these generated assets for app/window surfaces and installers; the tray inherits `app.default_window_icon()`. The native raster translates the in-app `lucide-react/Activity` mark into the same dark/lime identity at Windows icon sizes. All native icon inputs and outputs are local project files; no remote runtime asset or network dependency is required.

Voicel uses one outline icon family across the interface. Icons are imported from the local `lucide-react` package; no remote runtime asset is required.

| UI area | Icon | Source | Package/file | Size/stroke | Reason |
| --- | --- | --- | --- | --- | --- |
| Brand mark | Activity | Lucide | lucide-react/Activity | 17px, stroke 2.2 | Signals live listening without a fabricated logo mark |
| Primary navigation | Mic2 | Lucide | lucide-react/Mic2 | 18px, stroke 1.85 | Live dictation view |
| Primary navigation | History | Lucide | lucide-react/History | 18px, stroke 1.85 | Finished transcript view |
| Primary navigation | BookOpen | Lucide | lucide-react/BookOpen | 18px, stroke 1.85 | Custom vocabulary view |
| Primary navigation | Cpu | Lucide | lucide-react/Cpu | 18px, stroke 1.85 | Local model view |
| Primary navigation | SlidersHorizontal | Lucide | lucide-react/SlidersHorizontal | 18px, stroke 1.85 | Settings view |
| Navigation/state | ChevronRight | Lucide | lucide-react/ChevronRight | 15px, stroke 2 | Current view and route affordance |
| Status | CircleCheck | Lucide | lucide-react/CircleCheck | 15–20px, stroke 1.8–2 | Committed, selected, and successful states |
| Status | CircleAlert | Lucide | lucide-react/CircleAlert | 15–24px, stroke 1.8–2 | Recoverable error and availability states |
| Status | Loader2 | Lucide | lucide-react/Loader2 | 14–25px, stroke 1.7–2 | Honest pending/loading state |
| Recording | Volume2 | Lucide | lucide-react/Volume2 | 19px, stroke 1.7 | Signal inspector heading |
| Recording | Square | Lucide | lucide-react/Square | 14px, stroke 2.3 | Stop action |
| Recording | Zap | Lucide | lucide-react/Zap | 14–15px, stroke 1.8 | Keyboard/native behavior note |
| Clipboard actions | Copy | Lucide | lucide-react/Copy | 15–16px, stroke 2 | Copy transcript action |
| Clipboard actions | Clipboard | Lucide | lucide-react/Clipboard | 18px, stroke 1.8 | Text delivery settings |
| Vocabulary actions | Plus | Lucide | lucide-react/Plus | 16px, stroke 2 | Add custom word |
| Vocabulary actions | Save | Lucide | lucide-react/Save | 15–16px, stroke 1.9–2 | Save edited word/settings |
| Destructive actions | Trash2 | Lucide | lucide-react/Trash2 | 15–16px, stroke 1.9 | Clear/delete actions |
| Models | Download | Lucide | lucide-react/Download | 15–18px, stroke 1.8–2 | Install a local model |
| Settings | Settings2 | Lucide | lucide-react/Settings2 | 18px, stroke 1.8 | Startup settings section |
| Settings | Keyboard | Lucide | lucide-react/Keyboard | 15–18px, stroke 1.8 | Hotkey affordance and section |
| Utility | RefreshCw | Lucide | lucide-react/RefreshCw | 15–16px, stroke 2 | Retry/recovery action |
| Utility | X | Lucide | lucide-react/X | 15–16px, stroke 2 | Cancel/dismiss action |
| Utility | Minus | Lucide | lucide-react/Minus | 14px, stroke 2 | Minimize the custom-chrome window |
| Utility | Check | Lucide | lucide-react/Check | 13–18px, stroke 2–2.4 | Compact confirmation |
