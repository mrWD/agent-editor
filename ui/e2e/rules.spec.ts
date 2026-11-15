import { DEMO_FILE, expect, test, unique, type App } from './support/fixtures';

const openAgents = async (app: App) => {
  await app.page.getByRole('navigation', { name: 'Sections' }).getByRole('link', { name: 'Agents' }).click();
  await expect(app.page.getByRole('region', { name: 'Rules given to the agents' })).toBeVisible();
};
const rulesList = (app: App) => app.page.getByRole('region', { name: 'Rules given to the agents' });

test.describe('Rules of a project', () => {
  test('Z-01 Z-02 a rule written by hand is told to the agent with its first message, once', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.addProject(shop.dir);
    await app.openHome();
    await openAgents(app);
    const rule = `Use pnpm, never npm (${unique('r')}).`;
    await rulesList(app).getByRole('textbox', { name: 'A new rule' }).fill(rule);
    await rulesList(app).getByLabel('Where the rule applies').selectOption({ label: shop.name });
    await rulesList(app).getByRole('button', { name: 'Add the rule' }).click();
    const row = rulesList(app).locator('.rule-row', { hasText: rule });
    await expect(row).toContainText(shop.name);
    await expect(row).toContainText('Written by hand');
    // The same rule twice is refused.
    await rulesList(app).getByRole('textbox', { name: 'A new rule' }).fill(rule);
    await rulesList(app).getByRole('button', { name: 'Add the rule' }).click();
    await expect(app.page.getByRole('alert')).toContainText('That rule is on the list already');

    await app.goHome();
    await app.createTask({ text: unique('Describe the limiter'), project: shop.name });
    // The practice agent repeats what it was sent.
    // What the agent was sent is read from its thought, and a thought is shown only with every step.
    await app.allSteps();
    await expect(app.thoughts.first()).toContainText('the user has set these rules for this project');
    await expect(app.thoughts.first()).toContainText(`- ${rule}`);
    // What the user typed is shown as typed, without the note.
    await expect(app.messages.first()).not.toContainText('rules');
    await app.say('And who calls it?');
    await expect(app.thoughts.last()).not.toContainText('rules for this project');

    await openAgents(app);
    await rulesList(app)
      .getByRole('button', { name: `Remove the rule: ${rule}` })
      .click();
    await expect(rulesList(app).locator('.rule-row', { hasText: rule })).toHaveCount(0);
  });

  test('Z-03 Z-04 rewriting a line the agent wrote offers a rule; it is saved, put off, or never asked again', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await app.letAgentEdit('Change the limit');
    await app.openFile(DEMO_FILE);
    const card = app.conversation.getByRole('region', { name: 'Make this a rule?' });

    // The user's own line raises no question.
    await app.typeAtEndOf('windowMs: 60 * 1000,', ' // a minute');
    await app.saveWithKeyboard();
    await expect(card).toBeHidden();

    // The agent's line, rewritten, does.
    await app.typeAtEndOf('limit: limits.login,', ' // from config');
    await app.saveWithKeyboard();
    await expect(card).toContainText('You rewrote what Practice agent wrote in rateLimit.ts.');
    await expect(card).toContainText(`This project, ${shop.name}`);
    await expect(card.getByRole('button', { name: 'Save rule' })).toBeDisabled();
    const rule = `Never hardcode limits (${unique('r')}).`;
    await card.getByRole('textbox', { name: 'The rule, in one sentence' }).fill(rule);
    await card.getByRole('button', { name: 'Save rule' }).click();
    await expect(card).toBeHidden();

    await openAgents(app);
    const row = rulesList(app).locator('.rule-row', { hasText: rule });
    await expect(row).toContainText('From your correction');
    await expect(row).toContainText(shop.name);
  });

  test('Z-05 "Not this time" puts the question away; "Stop asking here" ends it for the project', async ({
    app,
    project,
  }) => {
    const shop = await project();
    await app.openHome();
    await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await app.letAgentEdit('Change the limit');
    await app.openFile(DEMO_FILE);
    const card = app.conversation.getByRole('region', { name: 'Make this a rule?' });

    await app.typeAtEndOf('limit: limits.login,', ' // one');
    await app.saveWithKeyboard();
    await card.getByRole('button', { name: 'Not this time' }).click();
    await expect(card).toBeHidden();

    // The agent writes again, the user corrects again: the question comes back, and is turned off for good.
    await app.letAgentEdit('Change it once more');
    await app.typeAtEndOf('Edited by the practice agent.', ' Really?');
    await app.saveWithKeyboard();
    await expect(card).toBeVisible();
    await card.getByRole('button', { name: 'Stop asking here' }).click();
    await expect(card).toBeHidden();

    await app.letAgentEdit('Change it a third time');
    await app.typeAtEndOf('Edited by the practice agent.', ' Again?');
    await app.saveWithKeyboard();
    await app.page.waitForTimeout(600);
    await expect(card).toBeHidden();
  });
});
