import { createServer, type Server } from 'node:http';
import type { AddressInfo } from 'node:net';
import { expect, test, type App } from './support/fixtures';

const TOKEN = 'sntryu_good';

const ISSUE = {
  id: '4001',
  shortId: 'SHOP-1A',
  title: 'TypeError: cart.user is undefined',
  culprit: 'submit(src/checkout/submit)',
  level: 'error',
  count: '1204',
  userCount: 87,
  firstSeen: '2026-11-01T08:00:00Z',
  lastSeen: '2026-11-13T17:40:12Z',
  permalink: 'https://acme.sentry.io/issues/4001/',
};

const EVENT = {
  tags: [{ key: 'release', value: '1.4.2' }],
  entries: [
    {
      type: 'exception',
      data: {
        values: [
          {
            type: 'TypeError',
            value: 'cart.user is undefined',
            stacktrace: {
              frames: [
                { filename: 'node_modules/express/lib/router.js', function: 'handle', lineNo: 280, inApp: false },
                {
                  filename: 'src/checkout/submit.ts',
                  function: 'submit',
                  lineNo: 42,
                  colNo: 17,
                  inApp: true,
                  context: [
                    [41, '  const cart = await load();'],
                    [42, '  return cart.user.id;'],
                  ],
                },
              ],
            },
          },
        ],
      },
    },
  ],
};

/** What the stand-in answers to each address it knows, for whoever shows the token. */
const ANSWERS: Record<string, unknown> = {
  '/api/0/organizations/acme/': { slug: 'acme' },
  '/api/0/organizations/acme/projects/': [
    { slug: 'shop-web', name: 'Shop web' },
    { slug: 'shop-api', name: 'Shop API' },
  ],
  '/api/0/projects/acme/shop-web/issues/': [ISSUE],
  '/api/0/organizations/acme/issues/4001/': ISSUE,
  '/api/0/organizations/acme/issues/4001/events/latest/': EVENT,
};

/** A server of the test's own that answers as Sentry does. The real one is never reached. */
async function startSentry(): Promise<{ url: string; server: Server }> {
  const server = createServer((request, response) => {
    const path = (request.url ?? '').split('?')[0]!;
    const known = ANSWERS[path];
    const status = request.headers.authorization !== `Bearer ${TOKEN}` ? 401 : known === undefined ? 404 : 200;
    response.writeHead(status, { 'content-type': 'application/json' });
    response.end(JSON.stringify(status === 200 ? known : { detail: 'no' }));
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  return { url: `http://127.0.0.1:${(server.address() as AddressInfo).port}`, server };
}

const card = (app: App) => app.page.getByRole('region', { name: 'Sentry', exact: true });

test.describe('Errors from Sentry', () => {
  test.use({ freshCore: true });
  let sentry: { url: string; server: Server };
  test.beforeEach(async () => {
    sentry = await startSentry();
  });
  test.afterEach(async () => {
    await new Promise((resolve) => sentry.server.close(resolve));
  });

  test('SE-01 SE-02 SE-03 Sentry is connected with a token that is tried, a project is joined, and an error becomes a task with its stack trace', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.addProject(shop.dir);
    await app.page.goto(`${app.core.url}/#/issues`);

    // A token Sentry does not take is said to be one, and nothing is connected.
    await card(app).getByLabel('Sentry organization').fill('acme');
    await card(app).getByLabel('Address of your own Sentry').fill(sentry.url);
    await card(app).getByLabel('Sentry auth token').fill('sntryu_bad');
    await card(app).getByRole('button', { name: 'Connect Sentry' }).click();
    await expect(app.page.getByRole('alert').filter({ hasText: 'Sentry did not accept the token' })).toBeVisible();
    await card(app).getByLabel('Sentry auth token').fill(TOKEN);
    await card(app).getByRole('button', { name: 'Connect Sentry' }).click();
    await expect(card(app)).toContainText(`Connected to Sentry, organization acme at ${sentry.url}.`);
    // The token is on this computer and nowhere on the page.
    await expect(app.page.locator('body')).not.toContainText(TOKEN);

    // Each project is joined to one of Sentry's, picked from the organization's own.
    const errors = app.page.getByRole('region', { name: `Errors of ${shop.name} in Sentry` });
    await expect(errors).toContainText('not joined to a project of Sentry yet');
    await errors.getByRole('button', { name: 'Pick its Sentry project' }).click();
    await errors.getByRole('combobox', { name: `Sentry project for ${shop.name}` }).selectOption('shop-web');
    await errors.getByRole('button', { name: 'Save' }).click();
    await expect(errors).toContainText('Errors in Sentry · shop-web · 1 unresolved');
    const row = errors.locator('.issue-row', { hasText: 'SHOP-1A' });
    await expect(row).toContainText('TypeError: cart.user is undefined');
    await expect(row).toContainText('1,204 times, 87 users');
    await expect(row.getByRole('link', { name: 'SHOP-1A' })).toHaveAttribute('href', ISSUE.permalink);

    // Started, it is a task whose first message is the error as a person would look it up.
    await row.getByRole('button', { name: 'Start a task for SHOP-1A' }).click();
    await expect(app.page).toHaveURL(/#\/task\/[^/]+\/code$/);
    const said = app.conversation.locator('.bubble').first();
    await expect(said).toContainText('This is Sentry issue SHOP-1A');
    await expect(said).toContainText('It happened 1204 times to 87 users');
    await expect(said).toContainText('at submit (src/checkout/submit.ts:42:17)');
    await expect(said).toContainText('return cart.user.id;');

    // On the list it has its task now, and "Without a task" leaves it out.
    await app.page.goto(`${app.core.url}/#/issues`);
    await expect(row.getByRole('link', { name: 'Open' })).toBeVisible();
    await expect(row.getByRole('button', { name: 'Start a task for SHOP-1A' })).toHaveCount(0);
    await app.page.getByRole('button', { name: 'Without a task' }).click();
    await expect(errors).toContainText('Every unresolved error has a task.');

    // Forgetting the token takes the lists away; the form is back.
    await card(app).getByRole('button', { name: 'Forget the token' }).click();
    await expect(card(app).getByRole('button', { name: 'Connect Sentry' })).toBeVisible();
    await expect(errors).toHaveCount(0);
  });
});
