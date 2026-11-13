import { DEMO_FILE, expect, test, unique, type App } from './support/fixtures';

/** The light theme, and the choice between it, the dark one and the computer's own. */

const DARK_GROUND = 'rgb(15, 17, 21)';
const LIGHT_GROUND = 'rgb(246, 246, 243)';

const html = (app: App) => app.page.locator('html');
const ground = (app: App) => app.page.evaluate(() => getComputedStyle(document.body).backgroundColor);
const choice = (app: App) => app.page.getByLabel('Theme', { exact: true });

async function openSettings(app: App) {
  await app.page.goto(`${app.core.url}/#/settings`);
  await expect(choice(app)).toBeVisible();
}

test.describe('The theme', () => {
  test('TH-01 TH-02 the editor is dark until Light is chosen in Settings, and the choice is there after a reload', async ({
    app,
  }) => {
    await openSettings(app);
    await expect(html(app)).toHaveAttribute('data-theme', 'dark');
    await expect(choice(app)).toHaveValue('dark');
    expect(await ground(app)).toBe(DARK_GROUND);

    await choice(app).selectOption({ label: 'Light' });
    await expect(html(app)).toHaveAttribute('data-theme', 'light');
    expect(await ground(app)).toBe(LIGHT_GROUND);
    // The browser is told too, so its own parts (scroll bars, the fields of a form) are light.
    await expect(app.page.locator('meta[name="color-scheme"]')).toHaveAttribute('content', 'light');

    await app.page.reload();
    await expect(html(app)).toHaveAttribute('data-theme', 'light');
    await expect(choice(app)).toHaveValue('light');
    expect(await ground(app)).toBe(LIGHT_GROUND);

    await choice(app).selectOption({ label: 'Dark' });
    await expect(html(app)).toHaveAttribute('data-theme', 'dark');
    expect(await ground(app)).toBe(DARK_GROUND);
  });

  test('TH-03 "As the computer" follows the system, and changes with it while the editor is open', async ({ app }) => {
    await app.page.emulateMedia({ colorScheme: 'dark' });
    await openSettings(app);
    await choice(app).selectOption({ label: 'As the computer' });
    await expect(html(app)).toHaveAttribute('data-theme', 'dark');
    await app.page.emulateMedia({ colorScheme: 'light' });
    await expect(html(app)).toHaveAttribute('data-theme', 'light');
    expect(await ground(app)).toBe(LIGHT_GROUND);
    // A choice of one's own is not moved by the computer.
    await choice(app).selectOption({ label: 'Dark' });
    await expect(html(app)).toHaveAttribute('data-theme', 'dark');
  });

  test('TH-04 the code and the terminal are in the theme too, and change with it without a reload', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.page.addInitScript(() => {
      if (!sessionStorage.getItem('theme-set')) localStorage.setItem('ae.theme', 'light');
      sessionStorage.setItem('theme-set', 'yes');
    });
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await app.openFile(DEMO_FILE);
    await app.openTerminal();

    const colours = () =>
      app.page.evaluate(() => {
        const of = (selector: string, what: 'color' | 'backgroundColor') => {
          const element = document.querySelector(selector);
          return element ? getComputedStyle(element)[what] : null;
        };
        return {
          code: of('.cm-editor', 'color'),
          // A word of the language, which has a colour of its own in each theme.
          keyword: [...document.querySelectorAll('.cm-line span')]
            .filter((span) => span.textContent === 'export')
            .map((span) => getComputedStyle(span).color)[0],
          terminal: of('.xterm-scrollable-element', 'backgroundColor'),
        };
      });

    expect(await colours()).toEqual({
      code: 'rgb(27, 29, 33)',
      keyword: 'rgb(168, 50, 122)',
      terminal: 'rgb(238, 238, 234)',
    });

    // Chosen in another tab of this browser: this one follows, the terminal included, with nothing reloaded.
    await app.page.evaluate(() => {
      localStorage.removeItem('ae.theme');
      window.dispatchEvent(new StorageEvent('storage', { key: 'ae.theme', newValue: null }));
    });
    await expect(html(app)).toHaveAttribute('data-theme', 'dark');
    await expect.poll(colours).toEqual({
      code: 'rgb(231, 233, 237)',
      keyword: 'rgb(227, 139, 183)',
      terminal: 'rgb(11, 13, 16)',
    });
  });
});
