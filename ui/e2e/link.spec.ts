import type { WebSocketRoute } from '@playwright/test';
import { expect, test, unique } from './support/fixtures';

test.describe('The link to the core', () => {
  test.use({ freshCore: true });

  test('A-05 a question that arrives while the page still reads the history of its task is not lost', async ({
    app,
    project,
    core,
  }) => {
    // The page talks to the core through this test, which holds back the page's request for the
    // history of a task until the agent of that task has asked its question.
    let release: (() => void) | null = null;
    await app.page.routeWebSocket(/\/ws$/, (socket) => {
      const toCore = socket.connectToServer();
      toCore.onMessage((message) => socket.send(message));
      socket.onMessage((message) => {
        const asksHistory =
          typeof message === 'string' && (JSON.parse(message) as { method?: string }).method === 'tasks.history';
        if (asksHistory && !release) release = () => toCore.send(message);
        else toCore.send(message);
      });
    });
    const shop = await project();
    const title = unique('Change the limit');
    await app.openHome();
    await app.page.getByRole('textbox', { name: 'New task' }).fill(title);
    await app.page.getByLabel('Project', { exact: true }).selectOption({ label: 'Another folder' });
    await app.page.getByLabel('Folder of the new project').fill(shop.dir);
    await app.page.getByRole('button', { name: 'Start', exact: true }).click();

    // The task exists and its agent already waits for an answer; the page has not read its history yet.
    await expect
      .poll(async () => (await core.request<{ state: string }[]>('tasks.list')).map((task) => task.state))
      .toEqual(['waiting_permission']);
    expect(release).not.toBeNull();
    release!();

    await expect(app.question).toBeVisible();
    await expect(app.conversation.getByText('Waiting for you', { exact: true })).toBeVisible();
    await expect(
      app.conversation.getByRole('heading', { name: 'Practice agent is waiting for your answer' }),
    ).toBeVisible();
    await app.goHome();
    await expect(app.page.getByRole('heading', { level: 1 })).toHaveText('One thing needs you');
    await expect(app.page.getByRole('article').filter({ hasText: title })).toContainText(
      'Practice agent asks before it acts',
    );
    await expect(app.rail.getByRole('link', { name: `${title}: Practice agent asks` })).toBeVisible();
  });

  test('A-06 when the core says events were dropped the page reads everything anew', async ({ app, project, core }) => {
    // The page talks to the core through this test, which can lose what the core sends, as a slow link would.
    let losing = false;
    let toPage: WebSocketRoute | null = null;
    await app.page.routeWebSocket(/\/ws$/, (socket) => {
      toPage = socket;
      const toCore = socket.connectToServer();
      socket.onMessage((message) => toCore.send(message));
      toCore.onMessage((message) => {
        const isEvent =
          typeof message === 'string' && typeof (JSON.parse(message) as { event?: unknown }).event === 'string';
        if (!(losing && isEvent)) socket.send(message);
      });
    });
    const shop = await project();
    const projectId = await app.addProject(shop.dir);
    await app.openHome();
    await expect(app.page.getByRole('heading', { level: 1 })).toHaveText('No tasks yet');

    // A task is made elsewhere while every event to this page is lost.
    losing = true;
    const title = unique('Describe the limiter');
    const made = await core.request<{ id: string }>('tasks.create', { projectId, agentId: 'mock', text: title });
    await expect.poll(async () => (await core.task(made.id)).state).toBe('idle');
    await expect(app.page.getByRole('heading', { level: 1 })).toHaveText('No tasks yet');
    await expect(app.page.getByRole('main').getByRole('link', { name: title })).toHaveCount(0);

    // The core notices that this page fell behind and tells it so. The page asks for everything again.
    losing = false;
    toPage!.send(JSON.stringify({ event: 'resync', data: {} }));
    await expect(app.stageRow('Your turn', title)).toContainText('The limiter allows five attempts a minute');
    await expect(app.page.getByRole('heading', { level: 1 })).toHaveText(
      'Nothing needs you, one thing waits for your next word',
    );
  });
});
