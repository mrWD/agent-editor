import { expect, test, unique, writeIn, type App, type Project } from './support/fixtures';

const MAIN_TF = [
  'resource "aws_db_instance" "orders" {',
  '  instance_class = "db.t3.micro"',
  '}',
  '',
  'resource "aws_s3_bucket" "exports" {',
  '  bucket = "shop-exports"',
  '}',
  '',
].join('\n');

const WEB_YAML = [
  'apiVersion: apps/v1',
  'kind: Deployment',
  'metadata:',
  '  name: web',
  'spec:',
  '  replicas: 2',
  '  template:',
  '    spec:',
  '      containers:',
  '        - name: web',
  '          image: shop/web:1.4',
  '          ports:',
  '            - containerPort: 8080',
  '',
].join('\n');

/** A shop that keeps its infrastructure next to its code: Terraform, a manifest, a Dockerfile and a chart. */
function withInfra(shop: Project) {
  shop.write('infra/main.tf', MAIN_TF);
  shop.write('k8s/web.yaml', WEB_YAML);
  shop.write('Dockerfile', 'FROM node:18-alpine\nEXPOSE 3000\n');
  shop.write('deploy/shop/Chart.yaml', 'name: shop\nversion: 1.0.0\n');
  shop.write('deploy/shop/values.yaml', "image:\n  tag: '1.4'\n");
  shop.git(['add', '.']);
  shop.git(['commit', '-q', '-m', 'Infrastructure']);
}

const kinds = (app: App) =>
  app.page.getByRole('region', { name: 'Preview', exact: true }).getByRole('group', { name: 'What to preview on' });

/**
 * A task with the Infra of the Preview on show. `change` is what the task did to its files before that.
 * Gives the view and the folder the task works in.
 */
async function openInfra(app: App, shop: Project, change: (folder: string) => void = () => {}) {
  await app.openHome();
  const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
  const folder = await app.folderOf(taskId);
  change(folder);
  await app.showView('Preview');
  await kinds(app).getByRole('button', { name: 'Infra', exact: true }).click();
  return { view: app.page.getByRole('region', { name: 'Preview', exact: true }), folder };
}

test.describe('The infrastructure a task changes', () => {
  test('IN-01 IN-02 IN-03 the changes to a Terraform file, a manifest and a Dockerfile are read tool by tool, the dangerous ones are marked, and they go to the agent', async ({
    app,
    project,
  }) => {
    const shop = await project();
    withInfra(shop);
    const before = { terraform: app.core.calls('terraform').length, helm: app.core.calls('helm').length };
    const { view, folder } = await openInfra(app, shop, (folder) => {
      // A bucket goes and the database grows; the app gets another image, more of it and a privileged
      // container; the Dockerfile moves to another base and opens a port.
      writeIn(
        folder,
        'infra/main.tf',
        'resource "aws_db_instance" "orders" {\n  instance_class = "db.t3.small"\n}\n\nresource "aws_sqs_queue" "mail" {\n  name = "mail"\n}\n',
      );
      writeIn(
        folder,
        'k8s/web.yaml',
        WEB_YAML.replace('replicas: 2', 'replicas: 4')
          .replace('shop/web:1.4', 'shop/web:1.5')
          .replace('          ports:', '          securityContext:\n            privileged: true\n          ports:'),
      );
      writeIn(folder, 'Dockerfile', 'FROM node:20-alpine\nEXPOSE 3000 9229\n');
    });

    const changes = view.getByRole('region', { name: 'What this task changes in the infrastructure' });
    await expect(changes.getByRole('heading', { level: 2 })).toHaveText(/^Infrastructure\s*3 changed$/);
    await expect(changes).toContainText(
      'The project has Terraform, 1 file · Kubernetes, 1 file · Helm, 2 files · Dockerfiles, 1 file · Compose, 1 file.',
    );
    await expect(changes.locator('.db-head .tag')).toHaveText('3 to look at twice');
    // Grouped by tool, in the same order every time.
    await expect(changes.getByRole('heading', { level: 3 })).toHaveText(['Terraform', 'Kubernetes', 'Dockerfiles']);

    const terraform = changes.getByRole('region', { name: 'Terraform' });
    await expect(terraform.locator('.db-file')).toHaveText(/^infra\/main\.tf\s*changed$/);
    await expect(terraform.locator('.db-steps > li')).toHaveText([
      /^Changes resource aws_db_instance\.orders: instance_class\s*It holds data: a change that replaces it loses what it holds\.$/,
      /^Removes resource aws_s3_bucket\.exports\s*Terraform destroys it on the next apply, with whatever it holds\.$/,
      /^Adds resource aws_sqs_queue\.mail$/,
    ]);
    await expect(terraform.locator('.db-steps > li.danger')).toHaveCount(2);

    const kubernetes = changes.getByRole('region', { name: 'Kubernetes' });
    await expect(kubernetes.locator('.db-steps > li')).toHaveText([
      /^Changes Deployment web$/,
      /^replicas\s*4, was 2$/,
      /^image\s*of web: shop\/web:1\.5, was shop\/web:1\.4$/,
      /^web runs privileged\s*A privileged container can do on its host whatever root can\.$/,
    ]);
    // The fields a deploy turns on are marked as such.
    await expect(kubernetes.locator('.infra-field')).toHaveText(['replicas', 'image']);
    await expect(kubernetes.locator('.db-steps > li.danger')).toHaveText(/web runs privileged/);

    const docker = changes.getByRole('region', { name: 'Dockerfiles' });
    await expect(docker.locator('.db-steps > li')).toHaveText([
      /^image\s*base node:20-alpine, was node:18-alpine$/,
      /^ports\s*exposes 3000, 9229, was 3000$/,
    ]);

    // Reading starts no tool.
    expect(app.core.calls('terraform').length).toBe(before.terraform);
    expect(app.core.calls('helm').length).toBe(before.helm);

    // The whole reading goes along with the next message.
    await changes.getByRole('button', { name: 'Show it to the agent' }).click();
    await expect(changes).toContainText('It goes along with your next message.');
    const card = app.conversation.getByRole('region', {
      name: 'From the preview: what this task changes in the infrastructure',
    });
    await expect(card).toContainText('infra/main.tf (changed)');
    await expect(card).toContainText('- Removes resource `aws_s3_bucket.exports`');
    await expect(card).toContainText('Look twice: Terraform destroys it on the next apply, with whatever it holds.');
    await expect(card).toContainText('- image of `web`: `shop/web:1.5`, was `shop/web:1.4`');
    await expect(card).toContainText('3 of these deserve a second look.');

    // Looked at again, the view has read what the task wrote meanwhile.
    writeIn(folder, 'compose.yaml', 'services:\n  db:\n    image: postgres:16\n    privileged: true\n');
    await app.showView('Code');
    await app.showView('Preview');
    await expect(changes.getByRole('heading', { level: 3 })).toHaveText([
      'Terraform',
      'Kubernetes',
      'Dockerfiles',
      'Compose',
    ]);
    await expect(changes.locator('.db-head .tag')).toHaveText(/ to look at twice$/);
  });

  test('IN-04 a check runs only when it is pressed, with a stand-in for the tool, and its output goes to the agent', async ({
    app,
    project,
  }) => {
    const shop = await project();
    withInfra(shop);
    const before = { terraform: app.core.calls('terraform').length, helm: app.core.calls('helm').length };
    const { view } = await openInfra(app, shop, (folder) => {
      writeIn(folder, 'deploy/shop/values.yaml', "image:\n  tag: '1.5'\n");
      writeIn(folder, 'infra/broken.tf', 'resorce "x" "y" {}\n');
    });
    const checks = view.getByRole('region', { name: 'Checks of the infrastructure' });
    await expect(checks).toContainText('nothing is ever applied');
    // The folders the task touched come first.
    await expect(checks.locator('.infra-check > .db-file')).toHaveText([
      /^terraform validate\s*in infra\s*changed by this task\s*Run$/,
      /^terraform plan\s*in infra\s*changed by this task\s*Run$/,
      /^helm template\s*in deploy\/shop\s*changed by this task\s*Run$/,
    ]);
    await expect(checks).toContainText('This folder was never set up: run terraform init in it first.');
    expect(app.core.calls('terraform').length).toBe(before.terraform);

    // The chart is rendered, and nothing else is started by that.
    await checks.getByRole('button', { name: 'Run helm template in deploy/shop' }).click();
    const rendered = checks.getByRole('region', { name: 'The run of helm template in deploy/shop' });
    await expect(rendered.locator('.tag')).toHaveText('Ended well', { timeout: 30_000 });
    await expect(rendered.locator('.api-url')).toHaveText(/^\$ .*helm template \.$/);
    await expect(rendered.getByLabel('What helm template in deploy/shop printed')).toContainText(
      '# Source: shop/templates/web.yaml',
    );
    expect(app.core.calls('helm').slice(before.helm)).toEqual(['template .']);
    expect(app.core.calls('terraform').length).toBe(before.terraform);

    // A folder that does not validate says why, and that goes to the agent.
    await checks.getByRole('button', { name: 'Run terraform validate in infra' }).click();
    const validated = checks.getByRole('region', { name: 'The run of terraform validate in infra' });
    await expect(validated.locator('.tag')).toHaveText('Ended with code 1', { timeout: 30_000 });
    await expect(validated.getByLabel('What terraform validate in infra printed')).toContainText(
      'Error: Unsupported block type',
    );
    await validated.getByRole('button', { name: 'Show it to the agent' }).click();
    const card = app.conversation.getByRole('region', {
      name: 'From the preview: terraform validate in infra failed',
    });
    await expect(card).toContainText('validate -no-color');
    await expect(card).toContainText('It ended with code 1.');

    // A plan never locks or refreshes the state, and nothing was ever applied.
    await checks.getByRole('button', { name: 'Run terraform plan in infra' }).click();
    const planned = checks.getByRole('region', { name: 'The run of terraform plan in infra' });
    await expect(planned.locator('.tag')).toHaveText('Ended well', { timeout: 30_000 });
    await expect(planned.getByLabel('What terraform plan in infra printed')).toContainText(
      'Plan: 1 to add, 1 to change, 1 to destroy.',
    );
    expect(app.core.calls('terraform').slice(before.terraform)).toEqual([
      'validate -no-color',
      'plan -lock=false -refresh=false -input=false -no-color',
    ]);

    // Left and come back to, the view shows the last run of each check.
    await app.showView('Code');
    await app.showView('Preview');
    await expect(validated.locator('.tag')).toHaveText('Ended with code 1');
    await expect(planned.locator('.tag')).toHaveText('Ended well');
  });

  test('IN-05 a check that goes on can be stopped, and what it had printed stays', async ({ app, project }) => {
    const shop = await project();
    withInfra(shop);
    // The stand-in takes its time over a plan in a folder that holds slow.tf.
    const { view } = await openInfra(app, shop, (folder) => writeIn(folder, 'infra/slow.tf', '# slow\n'));
    const checks = view.getByRole('region', { name: 'Checks of the infrastructure' });
    await checks.getByRole('button', { name: 'Run terraform plan in infra' }).click();
    const planned = checks.getByRole('region', { name: 'The run of terraform plan in infra' });
    await expect(planned.locator('.tag')).toHaveText('Running');
    await expect(planned.getByLabel('What terraform plan in infra printed')).toContainText('Plan: 1 to add', {
      timeout: 30_000,
    });
    await checks.getByRole('button', { name: 'Stop terraform plan in infra' }).click();
    await expect(checks.getByRole('button', { name: 'Run terraform plan in infra' })).toBeEnabled();
    await expect(planned.locator('.tag')).toHaveText('Stopped');
    await expect(planned.getByLabel('What terraform plan in infra printed')).toContainText('Plan: 1 to add');
  });

  test('IN-06 Infra is offered only for a project that has infrastructure files, and is there once it has one', async ({
    app,
    project,
  }) => {
    // A project with nothing but code: the compose file every test project has is taken out.
    const shop = await project();
    shop.git(['rm', '-q', 'compose.yaml']);
    shop.git(['commit', '-q', '-m', 'No services']);
    await app.openHome();
    const taskId = await app.createTask({ text: unique('Describe the limiter'), folder: shop.dir });
    await app.showView('Preview');
    await expect(kinds(app).getByRole('button', { name: 'Jobs', exact: true })).toBeVisible();
    // The core is asked when the view opens: its answer is given a moment to come.
    await app.page.waitForTimeout(600);
    await expect(kinds(app).getByRole('button', { name: 'Infra', exact: true })).toHaveCount(0);

    // The task writes the project's first Dockerfile: looked at again, the kind is there, and the file is read as new.
    writeIn(await app.folderOf(taskId), 'Dockerfile', 'FROM python:3.12-slim\nEXPOSE 8000\n');
    await app.showView('Code');
    await app.showView('Preview');
    const infra = kinds(app).getByRole('button', { name: 'Infra', exact: true });
    await expect(infra).toBeVisible();
    await infra.click();
    const changes = app.page.getByRole('region', { name: 'What this task changes in the infrastructure' });
    await expect(changes.getByRole('heading', { level: 2 })).toHaveText(/^Infrastructure\s*1 new$/);
    await expect(changes.locator('.db-file')).toHaveText(/^Dockerfile\s*new$/);
    await expect(changes.locator('.db-steps > li')).toHaveText([
      /^image\s*base python:3\.12-slim$/,
      /^ports\s*exposes 8000$/,
    ]);
    // No Terraform and no chart: there is nothing to check by hand.
    await expect(app.page.getByRole('region', { name: 'Checks of the infrastructure' })).toHaveCount(0);
  });
});
