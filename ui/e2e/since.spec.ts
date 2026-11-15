import { DEMO_FILE, expect, test, unique } from './support/fixtures';

type Event = { type: string; requestId?: string };

test.describe('Coming back to a task', () => {
  test('SN-01 SN-02 the task says where the user stopped and what happened since; Changes shows only what is new', async ({
    app,
    core,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    const first = unique('Describe the limiter');
    const taskId = await app.createTask({ text: first, folder: shop.dir });
    await app.askForEdit('Change the limit');

    // The user leaves while the agent waits. The answer is given elsewhere and the agent finishes.
    await app.goHome();
    // The core has noted the leaving, with the code as it was at that moment.
    await expect.poll(() => core.request('tasks.since', { taskId })).not.toBeNull();
    const { events } = await core.request<{ events: Event[] }>('tasks.history', { taskId });
    const requestId = events.findLast((event) => event.type === 'permission_request')!.requestId!;
    await core.request('tasks.permission', { taskId, requestId, optionId: 'allow-once' });
    await expect.poll(async () => (await core.task(taskId)).state).toBe('idle');

    await app.openTask(taskId);
    const card = app.conversation.getByRole('region', { name: 'Where you stopped' });
    await expect(card.getByRole('heading')).toHaveText(/^Where you stopped · today, \d\d:\d\d$/);
    await expect(card).toContainText(
      `You started this task with “${first}” and told Practice agent “Change the limit”, and left while Practice agent was waiting for your answer.`,
    );
    await expect(card.getByRole('listitem')).toHaveText([
      /^Practice agent changed rateLimit\.ts/,
      /^Practice agent finished its turn/,
    ]);
    await expect(card).toContainText(
      'Practice agent said: Done. The limit now comes from config instead of a fixed 5. One file changed.',
    );
    await expect(card).toContainText('Practice agent is paused, there is no merge request yet.');

    // From the card straight to what changed, and only that.
    await card.getByRole('link', { name: 'See what changed since' }).click();
    const away = app.page.getByRole('region', { name: 'What happened while you were away' });
    await expect(away.getByRole('button', { name: 'Since last visit' })).toHaveAttribute('aria-pressed', 'true');
    await expect(away).toContainText('Practice agent changed rateLimit.ts');
    await expect(away.getByRole('button', { name: new RegExp(`^${DEMO_FILE.replace(/\./g, '\\.')}`) })).toContainText(
      '+1−1',
    );
    await expect(app.diffRow('remove', 'limit: 5,')).toBeVisible();
    await expect(app.diffRow('add', 'limit: limits.login,')).toBeVisible();

    // "Mark all as seen" starts the count from here.
    await away.getByRole('button', { name: 'Mark all as seen' }).click();
    await expect(away).toContainText('No file has changed since your last visit.');
    await expect(app.diffRow('add', 'limit: limits.login,')).toBeHidden();

    // The parts of the view still show the change: it is not committed. And the way back is one press.
    await away.getByRole('button', { name: 'Not committed' }).click();
    await expect(app.changesTab('Not committed')).toHaveAttribute('aria-pressed', 'true');
    await expect(app.changesTab('Not committed')).toHaveText('Not committed1');
    await expect(app.diffRow('add', 'limit: limits.login,')).toBeVisible();
    await app.git.getByRole('button', { name: 'Since last visit' }).click();
    await expect(away.getByRole('button', { name: 'Since last visit' })).toHaveAttribute('aria-pressed', 'true');
    await expect(away).toContainText('No file has changed since your last visit.');
  });

  test('SN-03 the reminder can be hidden, goes away with the next message, and is not shown after a glance', async ({
    app,
    core,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    const card = app.conversation.getByRole('region', { name: 'Where you stopped' });

    // Nothing happened while away and the break was short: no reminder.
    await app.say('And who calls it?');
    await app.goHome();
    await expect.poll(() => core.request('tasks.since', { taskId })).not.toBeNull();
    await app.openTask(taskId);
    await expect(app.messages).toHaveCount(2);
    await expect(card).toBeHidden();

    // Something happens while the user is away: the reminder is there, and "Hide" takes it away.
    await app.goHome();
    await core.request('tasks.prompt', { taskId, text: 'One more question' });
    await expect.poll(async () => (await core.task(taskId)).state).toBe('idle');
    await app.openTask(taskId);
    await expect(card).toContainText('Practice agent finished its turn');
    await card.getByRole('button', { name: 'Hide' }).click();
    await expect(card).toBeHidden();

    // A glance at Home and back does not bring the same reminder up again.
    await app.goHome();
    await app.openTask(taskId);
    await expect(app.messages).toHaveCount(3);
    await app.page.waitForTimeout(700);
    await expect(card).toBeHidden();

    // News brings it back, and a message from the user makes it go away too.
    await app.goHome();
    await core.request('tasks.prompt', { taskId, text: 'And another one' });
    await expect.poll(async () => (await core.task(taskId)).state).toBe('idle');
    await app.openTask(taskId);
    await expect(card).toContainText('Practice agent finished 2 turns');
    await app.say('Thanks');
    await expect(card).toBeHidden();
  });
});
