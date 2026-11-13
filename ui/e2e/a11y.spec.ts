import AxeBuilder from '@axe-core/playwright';
import type { Page } from '@playwright/test';
import { DEMO_FILE, expect, test, unique } from './support/fixtures';

/**
 * Every screen is read by axe, the checker of the rules people who use a
 * screen reader or the keyboard depend on: names for every control, text
 * that stands out from its background, lists and headings that are what they
 * say they are. A screen passes when axe finds nothing.
 */
async function problemsOn(page: Page): Promise<string[]> {
  const found = await new AxeBuilder({ page })
    .withTags(['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa'])
    // The terminal and the code editor are other people's components with their own ways of being read.
    .exclude('.xterm')
    .exclude('.cm-editor')
    .analyze();
  return found.violations.flatMap((rule) =>
    rule.nodes.map((node) => `${rule.id}: ${rule.help} — ${node.target.join(' ')}`),
  );
}

test.describe('Every screen can be read and used without a mouse', () => {
  // Both themes are read: the same markup in other colours is another answer to "can this be read?".
  for (const theme of ['dark', 'light'] as const) {
    test(`T-01 T-02 T-03 the sections and the views of a task pass the accessibility rules, in the ${theme} theme`, async ({
      app,
      project,
      browserName,
    }) => {
      // axe reads the page's own markup and the styles it computes, which every engine builds the same way
      // from the same code. Reading all eleven screens three times over buys minutes and tells nothing new.
      test.skip(browserName !== 'chromium', 'the markup is the same in every engine');
      test.slow();
      // The choice of a theme is this browser's own: it is there before the page is.
      if (theme === 'light') await app.page.addInitScript(() => localStorage.setItem('ae.theme', 'light'));
      const shop = await project();
      await app.openHome();
      await expect(app.page.locator('html')).toHaveAttribute('data-theme', theme);
      expect.soft(await problemsOn(app.page), 'Home').toEqual([]);

      const taskId = await app.createTask({ text: unique('Change the limit'), folder: shop.dir, until: 'question' });
      // The conversation with a question in it, next to the code.
      await app.openFile(DEMO_FILE);
      expect.soft(await problemsOn(app.page), 'Code, with a question from the agent').toEqual([]);
      await app.answer('Allow this edit');
      await app.turnEnds();

      for (const view of ['Changes', 'Plan', 'Preview', 'Map', 'Checks'] as const) {
        await app.showView(view);
        // A view reads its data after it is shown: give what it shows a moment to settle.
        await app.page.waitForTimeout(400);
        expect.soft(await problemsOn(app.page), view).toEqual([]);
      }

      for (const section of ['Issues', 'Health', 'Automations', 'Agents'] as const) {
        await app.page.getByRole('navigation', { name: 'Sections' }).getByRole('link', { name: section }).click();
        await expect(app.page.getByRole('heading', { level: 1 })).toBeVisible();
        await app.page.waitForTimeout(400);
        expect.soft(await problemsOn(app.page), section).toEqual([]);
      }

      // The open dock and the search over everything.
      await app.openTask(taskId);
      await app.rail.getByRole('button', { name: 'Expand' }).click();
      await expect(app.dock).toBeVisible();
      expect.soft(await problemsOn(app.page), 'The open dock').toEqual([]);
      await app.dock.getByRole('button', { name: 'Collapse' }).click();
      await app.page.getByRole('button', { name: /^Search/ }).click();
      await expect(app.page.getByRole('dialog')).toBeVisible();
      expect.soft(await problemsOn(app.page), 'The search').toEqual([]);
    });
  }

  test('T-01 every control of a task is reached with Tab, and the one in focus is marked', async ({
    app,
    project,
    browserName,
  }) => {
    // Safari's Tab stops only at text boxes unless the user turned "highlight each item" on; Option+Tab stops everywhere.
    const tab = browserName === 'webkit' ? 'Alt+Tab' : 'Tab';
    const shop = await project();
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    // Walk the whole screen with Tab. Each stop must be something a person can see, with a mark around it.
    const stops = new Set<string>();
    for (let step = 0; step < 80; step++) {
      await app.page.keyboard.press(tab);
      const stop = await app.page.evaluate(() => {
        const el = document.activeElement as HTMLElement | null;
        if (!el || el === document.body) return null;
        const box = el.getBoundingClientRect();
        const style = getComputedStyle(el);
        const name = el.getAttribute('aria-label') || el.textContent?.trim().slice(0, 40) || el.tagName;
        return {
          name: `${el.tagName} ${name}`,
          seen: box.width > 0 && box.height > 0 && style.visibility !== 'hidden',
          // The terminal and the editor draw their own cursor; everything else gets the outline.
          marked:
            style.outlineStyle !== 'none' ||
            Boolean(el.closest('.xterm, .cm-editor')) ||
            el.matches('textarea, input, select'),
        };
      });
      if (!stop) continue;
      expect(stop.seen, `${stop.name} takes the focus but cannot be seen`).toBe(true);
      expect(stop.marked, `${stop.name} takes the focus with no mark`).toBe(true);
      stops.add(stop.name);
    }
    // The walk went through the header, the views, the file tree and the conversation.
    expect(stops.size).toBeGreaterThan(20);
  });
});
