import { test, expect } from '@playwright/test';

/**
 * Helper to read visible text from xterm.js terminal rows.
 * xterm uses a canvas renderer, so we read from the DOM row layer.
 */
async function getTerminalText(page: import('@playwright/test').Page): Promise<string> {
  return page.evaluate(() => {
    const rows = document.querySelectorAll('.xterm-rows > div');
    return Array.from(rows).map(r => r.textContent ?? '').join('\n');
  });
}

/**
 * Wait for boot to fully complete. Boot shows:
 * 1. Welcome banner (with "rust-bash")
 * 2. Auto-typed "agent ..." command + cached response
 * 3. Final prompt
 *
 * We wait for at least TWO prompts (the auto-type prompt + the final prompt),
 * which means boot is done and the terminal is ready for user input.
 */
async function waitForBoot(page: import('@playwright/test').Page): Promise<void> {
  await page.waitForFunction(() => {
    const rows = document.querySelectorAll('.xterm-rows > div');
    const text = Array.from(rows).map(r => r.textContent ?? '').join('\n');
    // Count prompt occurrences — boot creates at least 2 (auto-type + final).
    // The prompt contains "rust-bash:~$".
    const promptCount = (text.match(/rust-bash:~\$/g) || []).length;
    return promptCount >= 2;
  }, { timeout: 45000 });
}

test.describe('rust-bash website', () => {
  test.beforeEach(async ({ page }) => {
    await page.goto('/');
    // Wait for xterm.js terminal to render
    await page.waitForSelector('.xterm-rows', { timeout: 15000 });
  });

  test('renders welcome banner', async ({ page }) => {
    await waitForBoot(page);
    const text = await getTerminalText(page);
    expect(text).toContain('rust');
  });

  test('shows prompt after boot', async ({ page }) => {
    await waitForBoot(page);
    const text = await getTerminalText(page);
    expect(text).toContain('rust-bash');
  });

  test('can type and execute echo command', async ({ page }) => {
    await waitForBoot(page);

    const terminal = page.locator('.xterm-helper-textarea');
    await terminal.focus();
    await terminal.pressSequentially('echo hello world', { delay: 30 });
    await terminal.press('Enter');

    await page.waitForTimeout(2000);
    const text = await getTerminalText(page);
    expect(text).toContain('hello world');
  });

  test('can execute cat README.md', async ({ page }) => {
    await waitForBoot(page);

    const terminal = page.locator('.xterm-helper-textarea');
    await terminal.focus();
    await terminal.pressSequentially('cat README.md', { delay: 30 });
    await terminal.press('Enter');

    await page.waitForTimeout(2000);
    const text = await getTerminalText(page);
    expect(text).toContain('rust-bash');
  });

  test('can execute ls and see files', async ({ page }) => {
    await waitForBoot(page);

    const terminal = page.locator('.xterm-helper-textarea');
    await terminal.focus();
    await terminal.pressSequentially('ls', { delay: 30 });
    await terminal.press('Enter');

    await page.waitForTimeout(2000);
    const text = await getTerminalText(page);
    expect(text).toContain('README.md');
  });

  test('ctrl+c shows ^C and new prompt', async ({ page }) => {
    await waitForBoot(page);

    const terminal = page.locator('.xterm-helper-textarea');
    await terminal.focus();
    await terminal.pressSequentially('some partial', { delay: 30 });

    await page.keyboard.down('Control');
    await page.keyboard.press('c');
    await page.keyboard.up('Control');

    await page.waitForTimeout(500);
    const text = await getTerminalText(page);
    expect(text).toContain('^C');
  });

  test('pipes work correctly', async ({ page }) => {
    await waitForBoot(page);

    const terminal = page.locator('.xterm-helper-textarea');
    await terminal.focus();
    await terminal.pressSequentially('echo "hello world" | wc -w', { delay: 30 });
    await terminal.press('Enter');

    await page.waitForTimeout(2000);
    const text = await getTerminalText(page);
    expect(text).toContain('2');
  });

  test('OG meta tags are present', async ({ page }) => {
    const ogImage = await page.locator('meta[property="og:image"]').getAttribute('content');
    expect(ogImage).toBe('https://rustbash.dev/og-image.png');

    const twitterImage = await page.locator('meta[name="twitter:image"]').getAttribute('content');
    expect(twitterImage).toBe('https://rustbash.dev/og-image.png');
  });
});
