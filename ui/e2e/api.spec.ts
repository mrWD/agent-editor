import { createServer, type Server } from 'node:http';
import type { AddressInfo } from 'node:net';
import { expect, test, unique, writeIn, type App } from './support/fixtures';

const KEY = 'Bearer shop-key';

/**
 * The description of the shop's API. `ofTask` is how the task's branch has it: placing an
 * order needs a count, an order no longer says its total, and there is a new operation.
 */
function description(ofTask: boolean) {
  const order = {
    type: 'object',
    required: ofTask ? ['id'] : ['id', 'total'],
    properties: ofTask ? { id: { type: 'integer' } } : { id: { type: 'integer' }, total: { type: 'number' } },
  };
  const placing = {
    type: 'object',
    required: ofTask ? ['sku', 'count'] : ['sku'],
    properties: {
      sku: { type: 'string', example: 'tea-1' },
      ...(ofTask ? { count: { type: 'integer', minimum: 1 } } : {}),
    },
  };
  return {
    openapi: '3.1.0',
    info: { title: 'Shop', version: '0.3' },
    paths: {
      '/health': { get: { summary: 'Whether it is up', responses: { '200': {} } } },
      '/orders': {
        post: {
          summary: 'Place an order',
          requestBody: { required: true, content: { 'application/json': { schema: placing } } },
          responses: { '201': { content: { 'application/json': { schema: order } } } },
        },
      },
      '/orders/{id}': {
        get: {
          summary: 'One order',
          parameters: [
            { name: 'id', in: 'path', required: true, schema: { type: 'integer' } },
            { name: 'expand', in: 'query', schema: { type: 'string' } },
          ],
          responses: { '200': {}, '404': {} },
        },
      },
      ...(ofTask ? { '/gifts': { get: { summary: 'What can be given', responses: { '200': {} } } } } : {}),
    },
  };
}

/** A service of the test's own that describes itself the way FastAPI does and answers a few requests. */
async function startService(ofTask: boolean): Promise<{ address: string; server: Server }> {
  const server = createServer((request, response) => {
    const url = new URL(request.url ?? '/', 'http://shop');
    const say = (status: number, body: unknown) => {
      response.writeHead(status, { 'content-type': 'application/json', 'x-served-by': 'shop' });
      response.end(JSON.stringify(body));
    };
    let sent = '';
    request.on('data', (chunk) => (sent += chunk));
    request.on('end', () => {
      if (url.pathname === '/openapi.json') return say(200, description(ofTask));
      if (url.pathname === '/health') return say(200, { up: true });
      if (url.pathname === '/orders' && request.method === 'POST') {
        if (request.headers.authorization !== KEY) return say(401, { detail: 'Not signed in' });
        const asked = JSON.parse(sent || '{}') as { sku?: string; count?: number };
        return say(201, { id: 7, sku: asked.sku, count: asked.count });
      }
      const order = /^\/orders\/(\d+)$/.exec(url.pathname);
      if (order)
        return say(order[1] === '7' ? 200 : 404, { id: Number(order[1]), expand: url.searchParams.get('expand') });
      return say(404, { detail: 'Not found' });
    });
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  return { address: `127.0.0.1:${(server.address() as AddressInfo).port}`, server };
}

/** A task with the API of the Preview on show. */
async function openApi(app: App, folder: string) {
  await app.openHome();
  await app.createTask({ text: unique('Describe the limiter'), folder });
  await app.showView('Preview');
  const view = app.page.getByRole('region', { name: 'Preview', exact: true });
  await view
    .getByRole('group', { name: 'What to preview on' })
    .getByRole('button', { name: 'API', exact: true })
    .click();
  return view;
}

test.describe('The API of a task', () => {
  let task: { address: string; server: Server };
  let base: { address: string; server: Server };
  test.beforeEach(async () => {
    [task, base] = await Promise.all([startService(true), startService(false)]);
  });
  test.afterEach(async () => {
    await Promise.all([task, base].map((one) => new Promise((resolve) => one.server.close(resolve))));
  });

  test('API-01 API-02 the operations of the running service are listed, a request is sent and its answer is shown with the status and the time', async ({
    app,
    project,
  }) => {
    const view = await openApi(app, (await project()).dir);
    // Nothing runs and no file describes the API: that is said, and any request can still be written.
    await expect(view.getByRole('status')).toContainText('No description of the API was found among the files');
    const list = view.getByRole('navigation', { name: 'What the API offers' });
    await expect(list.locator('.api-row')).toHaveText(['Any request']);

    await view.getByLabel('Address of the service').fill(task.address);
    await view.getByLabel('Address of the service').press('Enter');
    await expect(view).toContainText('Shop 0.3, as the service gives it at /openapi.json');
    await expect(list.locator('.api-row')).toHaveText([
      /GET\s*\/gifts/,
      /GET\s*\/health/,
      /POST\s*\/orders/,
      /GET\s*\/orders\/\{id\}/,
      'Any request',
    ]);

    // The first operation is on show at once. A path with a blank is not sent before the blank is filled.
    const request = view.getByRole('region', { name: 'Request' });
    await list.getByRole('button', { name: /orders\/\{id\}/ }).click();
    await expect(request.getByRole('heading')).toContainText('GET/orders/{id}');
    await expect(request.getByRole('button', { name: 'Send' })).toBeDisabled();
    await expect(request).toContainText('Fill in id in the path.');
    await request.getByLabel('id').fill('7');
    await request.getByLabel('expand').fill('lines & buyer');
    await expect(request.getByLabel('Where the request goes')).toHaveText(
      `GET http://${task.address}/orders/7?expand=lines%20%26%20buyer`,
    );
    await request.getByRole('button', { name: 'Send' }).click();
    const answer = request.getByRole('region', { name: 'Answer' });
    await expect(answer.locator('.tag')).toHaveText('200 OK');
    await expect(answer).toContainText(/\d+ ms · \d+ B/);
    await expect(answer.getByLabel('Body of the answer')).toHaveText('{\n  "id": 7,\n  "expand": "lines & buyer"\n}');
    await answer.getByText('Headers of the answer').click();
    await expect(answer.locator('.api-headers')).toContainText('x-served-by');

    // An answer that says "no" is shown as an answer; what was typed for one operation waits while another is tried.
    await request.getByLabel('id').fill('8');
    await request.getByRole('button', { name: 'Send' }).click();
    await expect(answer.locator('.tag')).toHaveText('404 Not Found');
    await list.getByRole('button', { name: /health/ }).click();
    await request.getByRole('button', { name: 'Send' }).click();
    await expect(answer.getByLabel('Body of the answer')).toHaveText('{\n  "up": true\n}');
    await list.getByRole('button', { name: /orders\/\{id\}/ }).click();
    await expect(request.getByLabel('id')).toHaveValue('8');
    await expect(answer.locator('.tag')).toHaveText('404 Not Found');

    // A request the description does not know: any method, any path.
    await list.getByRole('button', { name: 'Any request' }).click();
    await request.getByLabel('Method').selectOption('DELETE');
    await request.getByLabel('Path of the request').fill('/nowhere');
    await request.getByRole('button', { name: 'Send' }).click();
    await expect(answer.locator('.tag')).toHaveText('404 Not Found');
    await expect(answer.getByLabel('Body of the answer')).toContainText('"detail": "Not found"');

    // Left and come back to, the view opens on the API with everything where it was.
    await app.showView('Code');
    await app.showView('Preview');
    await expect(request.getByLabel('Path of the request')).toHaveValue('/nowhere');
    await expect(answer.locator('.tag')).toHaveText('404 Not Found');
  });

  test('API-03 a body starts from the description, a header kept for the project goes with every request, and an answer is shown to the agent without it', async ({
    app,
    project,
  }) => {
    const view = await openApi(app, (await project()).dir);
    await view.getByLabel('Address of the service').fill(task.address);
    await view.getByLabel('Address of the service').press('Enter');
    const list = view.getByRole('navigation', { name: 'What the API offers' });
    const request = view.getByRole('region', { name: 'Request' });
    const answer = request.getByRole('region', { name: 'Answer' });
    await list.getByRole('button', { name: /POST/ }).click();
    await expect(request.getByLabel('Body')).toHaveValue('{\n  "count": 1,\n  "sku": "tea-1"\n}');

    // A body that is not JSON is not sent.
    await request.getByLabel('Body').fill('{sku: tea}');
    await expect(request.getByRole('button', { name: 'Send' })).toBeDisabled();
    await expect(request).toContainText('The body is not JSON');
    await request.getByLabel('Body').fill('{"sku": "tea-2", "count": 3}');
    await request.getByRole('button', { name: 'Send' }).click();
    await expect(answer.locator('.tag')).toHaveText('401 Unauthorized');

    // The key is kept once, for the project, and goes with every request from then on.
    await view.getByRole('button', { name: 'Set up' }).click();
    const setup = view.getByRole('form', { name: 'How the API is read' });
    await setup.getByLabel('Headers for every request').fill('Authorization shop-key');
    await setup.getByRole('button', { name: 'Save' }).click();
    await expect(view.getByRole('alert')).toContainText('is not a header');
    await setup.getByLabel('Headers for every request').fill(`Authorization: ${KEY}`);
    await setup.getByRole('button', { name: 'Save' }).click();
    await expect(setup).toHaveCount(0);
    await request.getByRole('button', { name: 'Send' }).click();
    await expect(answer.locator('.tag')).toHaveText('201 Created');
    await expect(answer.getByLabel('Body of the answer')).toContainText('"sku": "tea-2"');

    // Shown to the agent, the answer stands above the message box and goes with the next message, without the key.
    await answer.getByRole('button', { name: 'Show it to the agent' }).click();
    await expect(answer).toContainText('It goes along with your next message.');
    const card = app.conversation.getByRole('region', { name: 'From the preview: POST /orders answered 201' });
    await expect(card).toContainText('Authorization: (kept for the project, not shown)');
    await expect(card).not.toContainText('shop-key');
    await card.getByPlaceholder('Say what is wrong here').fill('the count is missing');
    await app.say('Why?');
    const said = app.conversation.locator('.bubble').last();
    await expect(said).toContainText('The user shows you this from the preview: POST /orders answered 201.');
    await expect(said).toContainText('They say: "the count is missing"');
    await expect(said).toContainText('Answered 201 Created');
    await expect(card).toHaveCount(0);
  });

  test('API-04 API-05 against a service that runs the base, what the task changed in the contract is weighed and its operations come first', async ({
    app,
    project,
  }) => {
    const view = await openApi(app, (await project()).dir);
    await view.getByLabel('Address of the service').fill(task.address);
    await view.getByLabel('Address of the service').press('Enter');
    const list = view.getByRole('navigation', { name: 'What the API offers' });
    await expect(list.locator('.api-row')).toHaveCount(5);

    // Without a base there is nothing to compare with, and the view says what to give.
    await view.getByRole('button', { name: 'Contract', exact: true }).click();
    const contract = view.getByRole('region', { name: 'What the task changes in the contract' });
    await expect(contract).toContainText('give the address of a service that runs the base branch');

    await view.getByRole('button', { name: 'Set up' }).click();
    const setup = view.getByRole('form', { name: 'How the API is read' });
    await setup.getByLabel('A service that runs the base branch').fill(base.address);
    await setup.getByRole('button', { name: 'Save' }).click();
    await expect(view.getByRole('button', { name: '2 breaking, 1 safe' })).toHaveAttribute('aria-pressed', 'true');
    await expect(contract.getByRole('heading', { level: 2 })).toHaveText(`2 breaking, 1 safe against ${base.address}`);
    await expect(contract.getByRole('listitem')).toHaveText([
      'POST /ordersThe body now needs count',
      'POST /ordersThe 201 answer no longer has total',
      'GET /giftsIt is new',
    ]);

    // Once there is something to compare with, the list holds only what the task touched, each marked.
    const scope = list.getByRole('group', { name: 'Which operations to list' });
    await expect(scope.getByRole('button', { name: 'Changed 2' })).toHaveAttribute('aria-pressed', 'true');
    await expect(list.locator('.api-row')).toHaveText([
      /GET\s*\/gifts\s*new/,
      /POST\s*\/orders\s*changed/,
      'Any request',
    ]);
    // The whole API is one press away: what the task touched stands first in it.
    await scope.getByRole('button', { name: 'All 4' }).click();
    await expect(scope.getByRole('button', { name: 'All 4' })).toHaveAttribute('aria-pressed', 'true');
    await expect(list.locator('.api-row')).toHaveText([
      /GET\s*\/gifts\s*new/,
      /POST\s*\/orders\s*changed/,
      /GET\s*\/health$/,
      /GET\s*\/orders\/\{id\}$/,
      'Any request',
    ]);
    // Pressed in the contract, an operation opens.
    await contract.getByRole('button', { name: 'POST /orders' }).first().click();
    const request = view.getByRole('region', { name: 'Request' });
    await expect(request.getByRole('heading')).toContainText('POST/orders');
    await expect(request.getByRole('list', { name: 'What the task changed here' }).getByRole('listitem')).toHaveText([
      'Breaks clientsThe body now needs count',
      'Breaks clientsThe 201 answer no longer has total',
    ]);
  });

  test('API-06 a description kept as a file is compared with the file where the branch started', async ({
    app,
    project,
  }) => {
    const shop = await project();
    shop.write('docs/openapi.json', JSON.stringify(description(false)));
    shop.git(['add', '.']);
    shop.git(['commit', '-q', '-m', 'Describe the API']);
    const view = await openApi(app, shop.dir);
    await expect(view).toContainText('Shop 0.3, read from docs/openapi.json');
    await expect(view.getByRole('button', { name: 'No change in the contract' })).toBeVisible();
    await expect(view.getByRole('status')).toContainText('No service of this task is running');
    // With nothing touched the whole API is listed; asked for what changed, the list says there is none.
    const scope = view.getByRole('group', { name: 'Which operations to list' });
    await expect(scope.getByRole('button', { name: 'All 3' })).toHaveAttribute('aria-pressed', 'true');
    await scope.getByRole('button', { name: 'Changed 0' }).click();
    await expect(view.getByRole('navigation', { name: 'What the API offers' })).toContainText(
      'This task changes no operation of the API.',
    );

    // The task rewrites the file: the contract says what changed, with nothing committed.
    const folder = await app.folderOf(app.taskId());
    writeIn(folder, 'docs/openapi.json', JSON.stringify(description(true)));
    await view.getByRole('button', { name: 'No change in the contract' }).click();
    const contract = view.getByRole('region', { name: 'What the task changes in the contract' });
    // The change on the disk is not told to anybody here; looking again is.
    await app.showView('Code');
    await app.showView('Preview');
    await expect(contract.getByRole('heading', { level: 2 })).toHaveText('2 breaking, 1 safe against main');
  });
});
