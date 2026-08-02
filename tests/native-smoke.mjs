import { execFileSync } from "node:child_process";
import { chromium } from "@playwright/test";

const endpoint = process.env.VOICEL_CDP_URL ?? "http://127.0.0.1:9222";
const browser = await chromium.connectOverCDP(endpoint);

const pages = browser.contexts().flatMap((context) => context.pages());
const main = pages.find((page) => !page.url().includes("overlay"));
if (!main) throw new Error("Voicel main window was not available over CDP");

const invoke = (command, args = {}) =>
  main.evaluate(
    ({ command, args }) => window.__TAURI_INTERNALS__.invoke(command, args),
    { command, args },
  );

const sleep = (milliseconds) =>
  new Promise((resolve) => setTimeout(resolve, milliseconds));

async function snapshot() {
  return invoke("app_snapshot");
}

async function waitFor(predicate, label, timeoutMs = 15_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const value = await snapshot();
    if (predicate(value)) return value;
    await sleep(100);
  }
  throw new Error(`Timed out waiting for ${label}`);
}

function sendChord({ withMainKey = false } = {}) {
  const source = `
using System;
using System.Runtime.InteropServices;
using System.Threading;
public static class VoicelSmokeInput {
  [DllImport("user32.dll")]
  public static extern void keybd_event(byte key, byte scan, uint flags, UIntPtr extra);
  public static void Run() {
    const uint Up = 0x0002;
    keybd_event(0x11, 0, 0, UIntPtr.Zero);
    Thread.Sleep(60);
    keybd_event(0x12, 0, 0, UIntPtr.Zero);
    Thread.Sleep(80);
    ${withMainKey ? "keybd_event(0x44, 0, 0, UIntPtr.Zero); Thread.Sleep(40); keybd_event(0x44, 0, Up, UIntPtr.Zero);" : ""}
    keybd_event(0x12, 0, Up, UIntPtr.Zero);
    Thread.Sleep(40);
    keybd_event(0x11, 0, Up, UIntPtr.Zero);
  }
}`;
  execFileSync("powershell.exe", [
    "-NoProfile",
    "-Command",
    `Add-Type -TypeDefinition '${source.replaceAll("'", "''")}'; [VoicelSmokeInput]::Run()`,
  ]);
}

const before = await snapshot();
const originalShortcut = before.settings.toggleShortcut;
const originalInsertionMethod = before.settings.insertionMethod;
const originalHistoryCount = before.history.length;

try {
  await invoke("update_settings", {
    patch: { toggleShortcut: "Ctrl+Alt" },
  });
  const saved = await snapshot();
  if (saved.settings.toggleShortcut !== "Ctrl+Alt") {
    throw new Error("Modifier-only shortcut did not round-trip through native settings");
  }

  sendChord();
  const started = await waitFor(
    (value) => value.phase !== "ready",
    "modifier-only shortcut to start a session",
  );
  if (!["loading", "recording", "finalizing"].includes(started.phase)) {
    throw new Error(`Unexpected phase after modifier-only shortcut: ${started.phase}`);
  }

  await invoke("cancel_recording");
  const cancelled = await waitFor(
    (value) => value.phase === "ready",
    "cancelled session to return to ready",
  );
  if (cancelled.history.length !== originalHistoryCount) {
    throw new Error("Cancelled global session changed History");
  }

  sendChord({ withMainKey: true });
  await sleep(800);
  const polluted = await snapshot();
  if (polluted.phase !== "ready") {
    await invoke("cancel_recording");
    throw new Error("Ctrl+Alt+D incorrectly triggered the Ctrl+Alt-only binding");
  }

  await main.getByRole("button", { name: "History", exact: true }).click();
  await main.waitForFunction(
    (count) => document.querySelectorAll(".recording-row").length === count,
    originalHistoryCount,
  );

  await main.getByRole("button", { name: "Settings", exact: true }).click();
  const delivery = main.locator("#paste-method");
  if ((await delivery.inputValue()) === "type") {
    await delivery.selectOption("paste");
    await main.getByRole("button", { name: "Save changes" }).click();
    await waitFor(
      (value) => value.settings.insertionMethod === "clipboard_receipt",
      "temporary paste delivery save",
    );
    await main.waitForFunction(
      () => document.querySelector("#paste-method")?.value === "paste",
    );
  }
  await delivery.selectOption("type");
  await main.waitForFunction(
    () => !document.querySelector('button[type="submit"]')?.disabled,
  );
  await main.getByRole("button", { name: "Save changes" }).click();
  await main.waitForFunction(
    () => document.querySelector("#paste-method")?.value === "type",
  );
  const typed = await snapshot();
  if (typed.settings.insertionMethod !== "typing") {
    throw new Error("Type delivery did not round-trip through the native UI");
  }

  console.log(
    JSON.stringify({
      modifierOnlyRoundTrip: true,
      exactChordStartedSession: true,
      extraMainKeyRejected: true,
      cancelledSessionPreservedHistory: true,
      nativeHistoryRows: originalHistoryCount,
      typeDeliveryRoundTrip: true,
    }),
  );
} finally {
  await invoke("update_settings", {
    patch: {
      toggleShortcut: originalShortcut,
      insertionMethod: originalInsertionMethod,
    },
  });
  await browser.close();
}
