import { test, expect } from '@playwright/test';

const INITIAL_AGENT_COMMAND = 'agent "is this the matrix?"';

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
 * 2. Auto-typed initial command
 * 3. Focus handed to the terminal after `await terminalUI.boot()`
 *
 * The focus handoff is the reliable signal that the shell is ready. The boot
 * flow only draws one visible prompt by default.
 */
async function waitForBoot(page: import('@playwright/test').Page): Promise<void> {
  await page.waitForFunction((initialCommand) => {
    const rows = document.querySelectorAll('.xterm-rows > div');
    const text = Array.from(rows).map(r => r.textContent ?? '').join('\n');
    const helper = document.querySelector('.xterm-helper-textarea');
    return (
      text.includes(initialCommand) &&
      helper instanceof HTMLElement &&
      document.activeElement === helper
    );
  }, INITIAL_AGENT_COMMAND, { timeout: 45000 });
}

async function clearPrefilledCommand(page: import('@playwright/test').Page) {
  const terminal = page.locator('.xterm-helper-textarea');
  await terminal.focus();
  for (let i = 0; i < INITIAL_AGENT_COMMAND.length; i++) {
    await terminal.press('Backspace');
  }
  return terminal;
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

    const terminal = await clearPrefilledCommand(page);
    await terminal.pressSequentially('echo hello world', { delay: 30 });
    await terminal.press('Enter');

    await page.waitForTimeout(2000);
    const text = await getTerminalText(page);
    expect(text).toContain('hello world');
  });

  test('tab completion before wasm is ready does not crash the terminal', async ({ page }) => {
    const pageErrors: string[] = [];
    page.on('pageerror', (error) => {
      pageErrors.push(error.message);
    });

    await page.route('**/*.wasm', async (route) => {
      await new Promise((resolve) => setTimeout(resolve, 10000));
      await route.continue();
    });

    await page.goto('/');
    await page.waitForSelector('.xterm-rows', { timeout: 15000 });
    await page.waitForSelector('#app.visible', { timeout: 15000 });
    await page.waitForTimeout(1500);

    const terminal = page.locator('.xterm-helper-textarea');
    await terminal.focus();
    await terminal.press('Tab');

    // This test intentionally focuses the terminal before boot completes, so the
    // generic waitForBoot() helper would become a false positive. Sleep past the
    // mocked 10s WASM delay instead, then verify the shell is still usable.
    await page.waitForTimeout(7000);

    await clearPrefilledCommand(page);
    await terminal.pressSequentially('pwd', { delay: 30 });
    await terminal.press('Enter');

    await expect.poll(() => getTerminalText(page), { timeout: 10000 }).toContain(
      '/home/user',
    );
    expect(pageErrors).toEqual([]);
  });

  test('shows a clean prompt when wasm fails to load', async ({ page }) => {
    const pageErrors: string[] = [];
    page.on('pageerror', (error) => {
      pageErrors.push(error.message);
    });

    await page.route('**/*.wasm', async (route) => {
      await route.abort();
    });

    await page.goto('/');
    await page.waitForSelector('.xterm-rows', { timeout: 15000 });
    await page.waitForSelector('#app.visible', { timeout: 15000 });

    await expect.poll(() => getTerminalText(page), { timeout: 15000 }).toContain(
      'Failed to load WASM',
    );

    const text = await getTerminalText(page);
    const promptCount = (text.match(/rust-bash:~\$/g) || []).length;
    expect(promptCount).toBeGreaterThanOrEqual(2);
    expect(text).toContain('Make sure you have run: ./scripts/build-wasm.sh');
    // Playwright reports the intentionally aborted fetch as a page error even
    // though the app catches it and recovers to a clean prompt.
    expect(pageErrors.filter((message) => message !== 'Failed to fetch')).toEqual([]);
  });

  test('can execute cat README.md', async ({ page }) => {
    await waitForBoot(page);

    const terminal = await clearPrefilledCommand(page);
    await terminal.pressSequentially('cat README.md', { delay: 30 });
    await terminal.press('Enter');

    await page.waitForTimeout(2000);
    const text = await getTerminalText(page);
    expect(text).toContain('rust-bash');
  });

  test('can execute ls and see files', async ({ page }) => {
    await waitForBoot(page);

    const terminal = await clearPrefilledCommand(page);
    await terminal.pressSequentially('ls', { delay: 30 });
    await terminal.press('Enter');

    await page.waitForTimeout(2000);
    const text = await getTerminalText(page);
    expect(text).toContain('README.md');
  });

  test('ctrl+c shows ^C and new prompt', async ({ page }) => {
    await waitForBoot(page);

    const terminal = await clearPrefilledCommand(page);
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

    const terminal = await clearPrefilledCommand(page);
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

  test('agent requests use the preview origin API endpoint', async ({ page }) => {
    await page.route('**/api/chat/completions', async (route) => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({
          id: 'chatcmpl-test',
          object: 'chat.completion',
          created: 0,
          model: 'gemini-2.5-flash',
          choices: [
            {
              index: 0,
              message: {
                role: 'assistant',
                content: 'stubbed agent reply',
              },
              finish_reason: 'stop',
            },
          ],
        }),
      });
    });

    await waitForBoot(page);

    const terminal = await clearPrefilledCommand(page);
    await terminal.pressSequentially('agent "hello"', { delay: 30 });
    await terminal.press('Enter');

    await expect.poll(() => getTerminalText(page), { timeout: 10000 }).toContain(
      'stubbed agent reply',
    );
  });
});
