import { chromium, expect, test } from "@playwright/test";

const CHROME = "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe";
const APP_URL = process.env.VOICEL_TEST_URL ?? "http://localhost:1420/";

test("demo toggle, cancel, and minimum-width reflow stay honest", async ({}, testInfo) => {
  const browser = await chromium.launch({ executablePath: CHROME });
  const page = await browser.newPage({
    viewport: { width: 1040, height: 700 },
  });
  try {
    await page.goto(APP_URL, { waitUntil: "networkidle" });
    await expect(page.getByText("DEMO · NO NATIVE AUDIO", { exact: true })).toBeVisible();
    await expect(page.getByText("Demo fallback", { exact: true })).toHaveCount(0);
    await expect(page.getByText("Local preview only.", { exact: false })).toHaveCount(0);
    await expect(
      page.getByText("Live English", { exact: true }),
    ).toHaveCount(1);
    await expect(
      page.getByRole("button", { name: "Change model" }),
    ).toBeVisible();
    await expect(page.getByText("Live English ready", { exact: true })).toHaveCount(0);
    await expect(page.getByText("Ready", { exact: true })).toHaveCount(0);
    await expect(page.getByText("Standby", { exact: true })).toHaveCount(0);
    await expect(
      page.getByText("Nothing is being recorded.", { exact: true }),
    ).toHaveCount(0);
    await expect(
      page.getByText("Toggle recording. No push-to-talk.", { exact: true }),
    ).toHaveCount(0);
    await expect(page.locator(".rail-shortcut")).toHaveCount(0);
    await expect(page.locator(".hotkey-readout")).toHaveCount(0);
    await expect(page.locator(".transcript-empty kbd")).toHaveCount(1);
    await expect(page.getByText(/to start a session\.$/)).toBeVisible();
    await page.screenshot({
      path: testInfo.outputPath("voicel-demo-1040x700.png"),
    });

    await page.keyboard.press("Control+Shift+Space");
    await expect(
      page.getByText("Listening", { exact: true }).first(),
    ).toBeVisible();
    await expect(page.getByRole("button", { name: "Cancel" })).toBeVisible();
    await expect(
      page.getByText("Live", { exact: true }).last(),
    ).not.toBeEmpty();

    await page.keyboard.press("Escape");
    await expect(
      page.getByText("Session cancelled", { exact: true }),
    ).toBeVisible();
    await expect(
      page.getByText("Ready", { exact: true }),
    ).toHaveCount(0);
    await expect(page.getByText("Live English", { exact: true })).toHaveCount(1);

    await page.getByRole("button", { name: "Settings" }).click();
    const scrollState = await page.locator(".content").evaluate((element) => {
      const panel = element as HTMLElement;
      const overflow = panel.scrollHeight > panel.clientHeight;
      panel.scrollTo({ top: panel.scrollHeight });
      return { overflow, scrollTop: panel.scrollTop };
    });
    expect(scrollState.overflow).toBe(true);
    expect(scrollState.scrollTop).toBeGreaterThan(0);

    const recordingShortcut = page.locator("#recording-hotkey");
    await recordingShortcut.click();
    await expect(recordingShortcut).toContainText("Press your shortcut");
    await recordingShortcut.press("Control+Alt");
    await expect(recordingShortcut).toContainText("Ctrl+Alt");

    const pasteMethod = page.locator("#paste-method");
    await pasteMethod.selectOption("type");
    await page.getByRole("button", { name: "Save changes" }).click();
    await expect(pasteMethod).toHaveValue("type");

    await page.setViewportSize({ width: 820, height: 580 });
    const minimumScrollState = await page.locator(".content").evaluate((element) => {
      const panel = element as HTMLElement;
      panel.scrollTo({ top: 0 });
      const overflow = panel.scrollHeight > panel.clientHeight;
      panel.scrollTo({ top: panel.scrollHeight });
      return { overflow, scrollTop: panel.scrollTop };
    });
    expect(minimumScrollState.overflow).toBe(true);
    expect(minimumScrollState.scrollTop).toBeGreaterThan(0);
    await expect(page.getByRole("button", { name: "Save changes" })).toBeVisible();
    const hasHorizontalOverflow = await page.evaluate(
      () => document.documentElement.scrollWidth > window.innerWidth,
    );
    expect(hasHorizontalOverflow).toBe(false);
    await page.screenshot({
      path: testInfo.outputPath("voicel-demo-820x580.png"),
    });
  } finally {
    await browser.close();
  }
});
