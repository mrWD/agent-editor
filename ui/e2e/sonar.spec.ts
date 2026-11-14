import { createServer, type Server } from 'node:http';
import type { AddressInfo } from 'node:net';
import { expect, test, type App } from './support/fixtures';

const TOKEN = 'squ_good';

/** A server of the test's own that answers as SonarQube does for one project, "acme:shop". */
async function startSonar(): Promise<{ url: string; server: Server }> {
  const login = `Basic ${Buffer.from(`${TOKEN}:`).toString('base64')}`;
  const server = createServer((request, response) => {
    const asked = new URL(request.url ?? '/', 'http://sonar');
    const answer = (status: number, body: unknown) => {
      response.writeHead(status, { 'content-type': 'application/json' });
      response.end(JSON.stringify(body));
    };
    const allowed = request.headers.authorization === login;
    if (asked.pathname === '/api/authentication/validate') return answer(200, { valid: allowed });
    if (!allowed) return answer(401, {});
    if (asked.pathname === '/api/qualitygates/project_status') {
      if (asked.searchParams.get('projectKey') !== 'acme:shop') return answer(404, {});
      return answer(200, { projectStatus: { status: 'OK', conditions: [] } });
    }
    if (asked.pathname === '/api/issues/search') {
      return answer(200, {
        paging: { total: 7 },
        issues: [
          {
            severity: 'CRITICAL',
            type: 'BUG',
            rule: 'typescript:S2259',
            component: 'acme:shop:src/middleware/rateLimit.ts',
            line: 3,
            message: 'The limit can be undefined here.',
          },
        ],
      });
    }
    if (asked.pathname === '/api/hotspots/search') return answer(200, { paging: { total: 0 }, hotspots: [] });
    return answer(404, {});
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  return { url: `http://127.0.0.1:${(server.address() as AddressInfo).port}`, server };
}

const card = (app: App) => app.page.getByRole('region', { name: 'SonarQube' });
const missing = (app: App) => app.page.getByRole('region', { name: 'What is missing' });

test.describe('What SonarQube found', () => {
  test.use({ freshCore: true });
  let sonar: { url: string; server: Server };
  test.beforeEach(async () => {
    sonar = await startSonar();
  });
  test.afterEach(async () => {
    await new Promise((resolve) => sonar.server.close(resolve));
  });

  test('SQ-01 SQ-02 SQ-03 SonarQube is connected, "Check now" reads what it found, and a finding starts a task with the places spelled out', async ({
    app,
    project,
  }) => {
    const shop = await project();
    shop.write('sonar-project.properties', 'sonar.projectKey=acme:shop\n');
    await app.addProject(shop.dir);
    await app.page.goto(`${app.core.url}/#/health`);

    // A token it does not take is said to be one. The right one connects, and is nowhere on the page.
    await card(app).getByLabel('Address of SonarQube').fill(sonar.url);
    await card(app).getByLabel('SonarQube token').fill('squ_bad');
    await card(app).getByRole('button', { name: 'Connect SonarQube' }).click();
    await expect(card(app).getByRole('alert')).toHaveText('SonarQube did not accept the token.');
    await card(app).getByLabel('SonarQube token').fill(TOKEN);
    await card(app).getByRole('button', { name: 'Connect SonarQube' }).click();
    await expect(card(app)).toContainText(`Connected to ${sonar.url}. Press Check now to read what it found.`);
    await expect(app.page.locator('body')).not.toContainText(TOKEN);
    // Which project of SonarQube the project is, is read from its own files.
    await expect(card(app)).toContainText('acme:shop');
    await expect(card(app)).toContainText('from its sonar-project.properties');
    await expect(missing(app)).not.toContainText('SonarQube');

    // "Check now" asks it, and what it found stands among the findings.
    await app.page.getByRole('button', { name: 'Check now' }).click();
    const found = missing(app).locator('.finding', { hasText: 'SonarQube has 7 open findings' });
    await expect(found).toContainText('Quality');
    await expect(found).toContainText(
      'src/middleware/rateLimit.ts:3: The limit can be undefined here. (typescript:S2259, critical) And 6 more.',
    );
    await expect(card(app)).toContainText(/Last read today, \d\d:\d\d\./);

    // Fix starts a task that is told the rule, the file and the line.
    await found.getByRole('button', { name: 'Fix' }).click();
    await expect(app.page).toHaveURL(/#\/task\/[^/]+\/code$/);
    const said = app.conversation.locator('.bubble').first();
    await expect(said).toContainText('SonarQube reports these open findings for its project acme:shop');
    await expect(said).toContainText('- src/middleware/rateLimit.ts:3: The limit can be undefined here.');

    // A key given by hand stands before the files' one; forgetting the token takes SonarQube's findings away.
    await app.page.goto(`${app.core.url}/#/health`);
    await card(app)
      .getByRole('button', { name: `Change the SonarQube key of ${shop.name}` })
      .click();
    await card(app).getByLabel(`SonarQube project key for ${shop.name}`).fill('acme:other');
    await card(app).getByRole('button', { name: 'Save' }).click();
    await expect(card(app)).toContainText('given here');
    await card(app).getByRole('button', { name: 'Forget the token' }).click();
    await expect(card(app).getByRole('button', { name: 'Connect SonarQube' })).toBeVisible();
    await expect(missing(app)).not.toContainText('SonarQube');
  });
});
