import { chromium } from "@playwright/test";

const endpoint = process.env.VOICEL_CDP_URL ?? "http://127.0.0.1:9222";
const browser = await chromium.connectOverCDP(endpoint);

const pages = browser.contexts().flatMap((context) => context.pages());
const main = pages.find((page) => !page.url().includes("overlay"));
const overlay = pages.find((page) => page.url().includes("overlay"));
if (!main) throw new Error("Voicel main window was not available over CDP");
if (!overlay) throw new Error("Voicel overlay window was not available over CDP");

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

  await invoke("start_recording");
  const acknowledged = await snapshot();
  if (acknowledged.phase !== "loading") {
    throw new Error(`Expected immediate loading phase, got ${acknowledged.phase}`);
  }
  await overlay.waitForFunction(
    () => document.body.innerText.includes("PREPARING"),
    undefined,
    { timeout: 1_500 },
  );
  const started = await waitFor(
    (value) => value.phase === "recording",
    "native session to finish loading",
  );
  if (started.phase !== "recording") {
    throw new Error(`Unexpected phase after native startup: ${started.phase}`);
  }
  for (let sample = 0; sample < 20; sample += 1) {
    await sleep(250);
    const sustained = await snapshot();
    if (sustained.phase !== "recording") {
      throw new Error(
        `Native session left recording during sustained decode: ${sustained.phase}`,
      );
    }
  }

  await invoke("cancel_recording");
  const cancelled = await waitFor(
    (value) => value.phase === "ready" || value.phase === "error",
    "cancelled session to become inactive",
  );
  if (
    cancelled.phase === "error" &&
    cancelled.error !== "Restore target window focus"
  ) {
    throw new Error(`Unexpected cancel error: ${cancelled.error}`);
  }
  if (cancelled.history.length !== originalHistoryCount) {
    throw new Error("Cancelled global session changed History");
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
      immediateLoadingAcknowledgement: true,
      nativeSessionReachedRecording: true,
      customWordDecodeStayedAlive: true,
      cancelledSessionPreservedHistory: true,
      nativeHistoryRows: originalHistoryCount,
      typeDeliveryRoundTrip: true,
    }),
  );
} finally {
  const ending = await snapshot();
  if (["loading", "recording", "finalizing"].includes(ending.phase)) {
    await invoke("cancel_recording");
    await waitFor(
      (value) => value.phase === "ready" || value.phase === "error",
      "test cleanup to leave the session inactive",
    );
  }
  await invoke("update_settings", {
    patch: {
      toggleShortcut: originalShortcut,
      insertionMethod: originalInsertionMethod,
    },
  });
  await browser.close();
}
