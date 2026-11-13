import { expect, type Locator, type Page } from '@playwright/test';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import type { Core } from './core';

/** The name the practice agent goes by on screen. It is the only agent the tests use. */
export const AGENT = 'Practice agent';
/** What the practice agent says when it was not asked to change anything. */
export const PLAIN_ANSWER =
  'The limiter allows five attempts a minute and answers 429 after that. Nothing was changed.';

let counter = 0;

/**
 * Adds a number to a text, so every task has a title of its own. Only digits:
 * the practice agent reads words such as "fix" or "edit" as a request to edit.
 */
export function unique(text: string): string {
  counter += 1;
  return `${text} ${Date.now() % 100000}${counter}`;
}

const escaped = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');

/**
 * The interface as a person uses it: the same few moves every test makes,
 * found by role, label and visible text.
 */
export class App {
  constructor(
    readonly page: Page,
    readonly core: Core,
  ) {}

  // ----- going places -----

  async openHome() {
    await this.page.goto(`${this.core.url}/#/`);
    await expect(this.page.getByRole('textbox', { name: 'New task' })).toBeVisible();
  }

  async goHome() {
    await this.page.getByRole('link', { name: /^Home/ }).click();
    await expect(this.page.getByRole('textbox', { name: 'New task' })).toBeVisible();
  }

  async openTask(taskId: string, view = 'code') {
    await this.page.goto(`${this.core.url}/#/task/${taskId}/${view}`);
    await expect(this.conversation).toBeVisible();
  }

  /** The id of the task on screen, read from the address. */
  taskId(): string {
    const found = /#\/task\/([^/]+)/.exec(this.page.url());
    if (!found) throw new Error(`No task is on screen: ${this.page.url()}`);
    return found[1]!;
  }

  /** Picks one of the views of the task on screen: Code, Changes and so on. */
  async showView(name: 'Code' | 'Changes' | 'Plan' | 'Preview' | 'Map' | 'Checks') {
    const views = this.page.getByRole('navigation', { name: 'Views of this task' });
    await views.getByRole('link', { name: new RegExp(`^${name}`) }).click();
    await expect(views.getByRole('link', { name: new RegExp(`^${name}`) })).toHaveAttribute('aria-current', 'page');
  }

  // ----- Home -----

  /** The part of Home under one title, such as "Your turn". */
  stage(title: string): Locator {
    return this.page.locator('section', {
      has: this.page.getByRole('heading', { name: new RegExp(`^${escaped(title)}( \\d+)?$`) }),
    });
  }

  /** The row of one task in a stage of Home. */
  stageRow(title: string, taskTitle: string): Locator {
    return this.stage(title).locator('.stage-row', { hasText: taskTitle });
  }

  /**
   * Creates a task through the form on Home. `folder` adds a folder as a new
   * project; `project` picks one that is there already. Returns the task's id.
   * By default it waits for the end of the first turn; a first message that
   * asks for an edit stops at the agent's question instead.
   */
  async createTask(options: {
    text: string;
    folder?: string;
    project?: string;
    until?: 'turn' | 'question' | 'started';
  }): Promise<string> {
    const { page } = this;
    await page.getByRole('textbox', { name: 'New task' }).fill(options.text);
    const projects = page.getByLabel('Project', { exact: true });
    if (options.folder) {
      await projects.selectOption({ label: 'Another folder' });
      await page.getByLabel('Folder of the new project').fill(options.folder);
    } else if (options.project) {
      await projects.selectOption({ label: options.project });
    }
    await page.getByLabel('Agent', { exact: true }).selectOption({ label: AGENT });
    await page.getByRole('button', { name: 'Start', exact: true }).click();
    await page.waitForURL(/#\/task\/[^/]+\/code$/);
    await expect(this.conversation).toBeVisible();
    const until = options.until ?? 'turn';
    if (until === 'turn') await this.turnEnds();
    if (until === 'question') await expect(this.question).toBeVisible();
    return this.taskId();
  }

  // ----- the conversation -----

  get conversation(): Locator {
    return this.page.getByRole('complementary', { name: `Conversation with ${AGENT}` });
  }

  get messageBox(): Locator {
    return this.conversation.getByRole('textbox', { name: `Message ${AGENT}` });
  }

  get sendButton(): Locator {
    return this.conversation.getByRole('button', { name: 'Send', exact: true });
  }

  /** What the agent said, one entry per answer. */
  get answers(): Locator {
    return this.conversation.getByRole('article').getByRole('paragraph');
  }

  /** What the user said, one entry per message. */
  get messages(): Locator {
    return this.conversation.locator('.bubble');
  }

  /** What the agent thought aloud. The practice agent repeats there what it was sent. */
  get thoughts(): Locator {
    return this.conversation.locator('.thought');
  }

  /** The one-line entries of the conversation: who did what to which file. */
  get activities(): Locator {
    return this.conversation.locator('.activity');
  }

  /** The card with the agent's open question. */
  get question(): Locator {
    return this.conversation.getByRole('heading', { name: new RegExp(`^${AGENT} asks:`) });
  }

  /** Waits until the agent has finished its turn and it is the user's turn again. */
  async turnEnds() {
    await expect(this.conversation.getByText(`${AGENT} finished this turn.`)).toBeVisible();
  }

  /** Sends a message that asks for no edit and waits for the whole answer. */
  async say(text: string) {
    const before = await this.answers.count();
    await this.messageBox.fill(text);
    await this.sendButton.click();
    await expect(this.answers).toHaveCount(before + 1);
    await this.turnEnds();
  }

  /** Sends a message that asks for an edit and waits for the agent's question. */
  async askForEdit(text: string) {
    await this.messageBox.fill(text);
    await this.sendButton.click();
    await expect(this.question).toBeVisible();
  }

  /** Presses one of the answers to the agent's question, in the conversation. */
  async answer(option: 'Allow this edit' | 'Always allow edits' | 'Do not edit') {
    await this.conversation.getByRole('button', { name: option, exact: true }).click();
    await expect(this.question).toBeHidden();
  }

  /** Asks for an edit, allows it and waits for the end of the turn. */
  async letAgentEdit(text: string) {
    await this.askForEdit(text);
    await this.answer('Allow this edit');
    await this.turnEnds();
  }

  // ----- files and the editor -----

  get files(): Locator {
    return this.page.getByRole('region', { name: 'Files of this task' });
  }

  get editor(): Locator {
    return this.page.getByRole('textbox', { name: 'Code' });
  }

  /** The tab of an open file, by the file's name. */
  tab(fileName: string): Locator {
    return this.page.getByRole('tab', { name: new RegExp(`^${escaped(fileName)}`) });
  }

  /** The line under the editor: where the caret is and whether the file is saved. */
  get saved(): Locator {
    return this.page.getByText('Saved', { exact: true });
  }

  get notSaved(): Locator {
    return this.page.getByText(/^Not saved ·/);
  }

  /** Opens a file from the tree, folder by folder. */
  async openFile(file: string) {
    const parts = file.split('/');
    const name = parts.pop()!;
    for (const folder of parts) {
      const row = this.files.getByRole('button', { name: folder, exact: true });
      await expect(row).toBeVisible();
      if ((await row.getAttribute('aria-expanded')) !== 'true') await row.click();
    }
    await this.files.getByRole('button', { name: new RegExp(`^${escaped(name)}`) }).click();
    await expect(this.tab(name)).toHaveAttribute('aria-selected', 'true');
  }

  /** Puts the caret at the end of the line that holds `lineText` and types. */
  async typeAtEndOf(lineText: string, text: string) {
    await this.editor.getByText(lineText).first().click();
    await this.page.keyboard.press('End');
    await this.page.keyboard.type(text);
  }

  async saveWithKeyboard() {
    await this.page.keyboard.press('ControlOrMeta+s');
    await expect(this.saved).toBeVisible();
  }

  /** Tells the page that its window was looked at again, which is when it reads the open files anew. */
  async comeBackToWindow() {
    await this.page.evaluate(() => window.dispatchEvent(new Event('focus')));
  }

  // ----- the panel under the code -----

  get bottom(): Locator {
    return this.page.getByRole('region', { name: 'Terminal, run and problems' });
  }

  async openBottom(tab: 'Terminal' | 'Run' | 'Problems') {
    // A tab may carry a count after its name: "Run 1 of 2", "Problems 3".
    const button = this.bottom.getByRole('button', { name: new RegExp(`^${tab}( \\d|$)`) });
    if ((await button.getAttribute('aria-pressed')) !== 'true') await button.click();
    await expect(button).toHaveAttribute('aria-pressed', 'true');
  }

  /** The text the user's terminal shows. */
  get terminal(): Locator {
    return this.bottom.locator('.terminal .xterm-rows').first();
  }

  /** Opens the terminal and waits for the shell's prompt. */
  async openTerminal() {
    await this.openBottom('Terminal');
    await expect(this.terminal).toHaveText(/\S/);
  }

  /** Types a line into the terminal and presses Enter. */
  async runInTerminal(command: string) {
    await this.bottom.getByRole('textbox', { name: 'Terminal input' }).first().focus();
    await this.page.keyboard.type(command);
    await this.page.keyboard.press('Enter');
  }

  /** The table of what the project can run. */
  get services(): Locator {
    return this.bottom.getByRole('table', { name: 'Services of this task' });
  }

  service(name: string): Locator {
    return this.services.getByRole('row', { name: new RegExp(`^${escaped(name)} `) });
  }

  // ----- the search over everything -----

  get palette(): Locator {
    return this.page.getByRole('dialog', { name: 'Search everything' });
  }

  get searchBox(): Locator {
    return this.palette.getByRole('searchbox');
  }

  async openSearch() {
    await this.page.getByRole('button', { name: /^Search/ }).click();
    await expect(this.searchBox).toBeFocused();
  }

  // ----- Changes -----

  get git(): Locator {
    return this.page.getByRole('region', { name: 'Git: what goes in, commit, push' });
  }

  get request(): Locator {
    return this.page.getByRole('region', { name: 'Merge request' });
  }

  /** A row of the diff on screen, by a piece of its text. */
  diffRow(kind: 'add' | 'remove' | 'context', text: string): Locator {
    return this.page.locator(`.diff-row.${kind}`, { hasText: text });
  }

  /** One commit in "On this branch", by its subject. */
  commitRow(subject: string): Locator {
    return this.git.locator('.git-commit', { hasText: subject });
  }

  /** Commits what is ticked, with a message of its own. */
  async commit(message: string) {
    await this.git.getByRole('textbox', { name: 'Commit message' }).fill(message);
    await this.git.getByRole('button', { name: /^(Commit|Add) \d+ files?/ }).click();
    await expect(this.commitRow(message)).toBeVisible();
  }

  // ----- the projects rail -----

  get rail(): Locator {
    return this.page.getByRole('region', { name: 'Projects and the state of each task' });
  }

  get dock(): Locator {
    return this.page.getByRole('region', { name: 'Projects and their tasks' });
  }

  /** The card of a project's tasks that shows while the pointer or the focus is on the project. */
  railCard(projectName: string): Locator {
    return this.page.getByRole('region', { name: new RegExp(`^${escaped(projectName)}, shown while`) });
  }

  // ----- setting things up -----

  /** Makes a folder a project without starting a task in it. Returns the project's id. */
  async addProject(folder: string): Promise<string> {
    return (await this.core.request<{ id: string }>('projects.add', { path: folder })).id;
  }

  // ----- the disk -----

  /** The folder a task works in. */
  async folderOf(taskId: string): Promise<string> {
    return (await this.core.task(taskId)).cwd;
  }
}

/** Reads a file of a task's folder; empty when it is not there. */
export function readIn(folder: string, file: string): string {
  const target = path.join(folder, file);
  return existsSync(target) ? readFileSync(target, 'utf8') : '';
}

/** Changes a file of a task's folder from outside, as another program would. */
export function writeIn(folder: string, file: string, text: string) {
  writeFileSync(path.join(folder, file), text);
}
