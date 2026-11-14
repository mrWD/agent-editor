import { expect, test, unique, type App } from './support/fixtures';

const section = (app: App) => app.page.getByRole('region', { name: 'Tool servers' });
const row = (app: App, name: string) => section(app).getByRole('group', { name, exact: true });

test.describe('Tool servers of the user’s own', () => {
  test.use({ freshCore: true });

  test('TS-01 TS-02 TS-03 a server is picked ready-made, filled in, turned off and removed; what is wrong is said', async ({
    app,
  }) => {
    await app.page.goto(`${app.core.url}/#/settings`);
    await expect(section(app)).toContainText('None yet. Pick one that is ready, or add your own.');

    // One that is ready opens the form filled in, and says what is left to give.
    await section(app).getByRole('button', { name: 'Pick one that is ready' }).click();
    const ready = section(app).getByRole('list', { name: 'Tool servers that are ready to be filled in' });
    await expect(ready.getByRole('listitem').first()).toContainText('Sentry');
    await ready.getByRole('button', { name: 'Fill in Sentry with a token' }).click();
    const form = section(app).getByRole('form', { name: 'A new tool server' });
    await expect(form.getByLabel('Name: what agents see it as')).toHaveValue('Sentry');
    await expect(form.getByLabel('Program')).toHaveValue('npx');
    await expect(form.getByLabel('Arguments, as on a command line')).toHaveValue('-y @sentry/mcp-server@latest');
    await expect(form.getByLabel('Environment: one NAME=value on a line')).toHaveValue('SENTRY_ACCESS_TOKEN=');
    await expect(form).toContainText('Paste a user auth token of Sentry as SENTRY_ACCESS_TOKEN.');
    await form.getByRole('button', { name: 'Add the server' }).click();

    // It is listed by its command. Its key is named, never shown, and said to be missing.
    const sentry = row(app, 'Sentry');
    await expect(sentry).toContainText('A program');
    await expect(sentry).toContainText('npx -y @sentry/mcp-server@latest');
    await expect(sentry).toContainText('SENTRY_ACCESS_TOKEN is still empty: it will not work until you fill it in.');
    await sentry.getByRole('button', { name: 'Change Sentry' }).click();
    const change = section(app).getByRole('form', { name: 'The tool server Sentry' });
    await change.getByLabel('Environment: one NAME=value on a line').fill('SENTRY_ACCESS_TOKEN=sntryu_secret');
    await change.getByRole('button', { name: 'Save the server' }).click();
    await expect(sentry).toContainText('With SENTRY_ACCESS_TOKEN set.');
    await expect(sentry).not.toContainText('still empty');
    await expect(section(app)).not.toContainText('sntryu_secret');

    // One of the user's own, at an address. An address that is none is refused in words, and the form stays.
    await section(app).getByRole('button', { name: 'Add your own' }).click();
    await form.getByLabel('Name: what agents see it as').fill('Wiki');
    await form.getByLabel('How it is reached').selectOption('http');
    await form.getByLabel('Address').fill('wiki.example.com/mcp');
    await form.getByRole('button', { name: 'Add the server' }).click();
    await expect(section(app).getByRole('alert')).toContainText(
      "Give the server's address, beginning with https:// or http://",
    );
    await form.getByLabel('Address').fill('https://wiki.example.com/mcp');
    await form.getByLabel('Headers sent with every request').fill('Authorization: Bearer abc');
    await form.getByRole('button', { name: 'Add the server' }).click();
    await expect(row(app, 'Wiki')).toContainText('https://wiki.example.com/mcp');
    await expect(row(app, 'Wiki')).toContainText('With Authorization set.');
    await expect(section(app).getByRole('alert')).toHaveCount(0);

    // Off keeps it in the list; removing takes two presses.
    const hand = row(app, 'Wiki').getByRole('button', { name: 'Hand Wiki to agents' });
    await expect(hand).toHaveAttribute('aria-pressed', 'true');
    await hand.click();
    await expect(hand).toHaveAttribute('aria-pressed', 'false');
    await expect(row(app, 'Wiki')).toContainText('Off');
    await row(app, 'Wiki').getByRole('button', { name: 'Remove' }).click();
    await expect(row(app, 'Wiki')).toBeVisible();
    await row(app, 'Wiki').getByRole('button', { name: 'Press again to remove' }).click();
    await expect(row(app, 'Wiki')).toHaveCount(0);
    // The list is the core's: it is there after a reload.
    await app.page.reload();
    await expect(row(app, 'Sentry')).toBeVisible();
  });

  test('TS-04 an agent that starts is named the servers that are on', async ({ app, project }) => {
    const shop = await project();
    await app.core.request('tools.save', { name: 'Sentry', kind: 'http', url: 'https://mcp.sentry.dev/mcp' });
    await app.core.request('tools.save', { name: 'Db', kind: 'stdio', program: 'ae-no-such-program', on: false });
    await app.openHome();
    await app.createTask({ text: unique('Which tool servers do you have?'), folder: shop.dir });
    await expect(app.answers.last()).toContainText(
      'This session was given these tool servers: Sentry (http https://mcp.sentry.dev/mcp; headers: ).',
    );
  });
});
