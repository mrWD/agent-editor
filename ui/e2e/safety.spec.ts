import { execFileSync } from 'node:child_process';
import { realpathSync } from 'node:fs';
import { networkInterfaces, tmpdir } from 'node:os';
import path from 'node:path';
import { expect, test } from './support/fixtures';

type ToolState = { installed: boolean; signedIn: boolean; hint: string | null };
type Integrations = { github: ToolState; gitlab: ToolState; docker: ToolState; linear: { hasKey: boolean } };

test.describe('the test setup itself', () => {
  test('the core runs on stand-ins and cannot reach a real service', async ({ core }) => {
    // What the core was started with.
    expect(core.env.PATH.split(':')[0]).toBe(core.toolsDir);
    expect(core.env).not.toHaveProperty('LINEAR_API_KEY');
    expect(core.env.AE_LINEAR_URL).toBe('http://127.0.0.1:9/graphql');
    expect(core.port).not.toBe(4477);
    expect(core.dataDir.startsWith(realpathSync(tmpdir()) + path.sep)).toBe(true);

    // What the running process really has.
    const seen = execFileSync('ps', ['eww', '-o', 'command=', '-p', String(core.pid)], { encoding: 'utf8' });
    expect(seen).toContain(`PATH=${core.toolsDir}:`);
    expect(seen).toContain('AE_LINEAR_URL=http://127.0.0.1:9/graphql');
    expect(seen).not.toContain('LINEAR_API_KEY');
    expect(seen).toContain(`--data-dir ${core.dataDir}`);

    // Asking the core what is connected makes it call the tools: the stand-ins are what answers.
    const status = await core.request<Integrations>('integrations.status');
    expect(core.calls('gh')).toContain('auth status');
    expect(core.calls('glab')).toContain('auth status');
    expect(core.calls('docker').some((call) => call.startsWith('info'))).toBe(true);
    expect(status.github).toEqual({ installed: true, signedIn: true, hint: null });
    // The stand-in for glab always says "not signed in", whatever the real one would say.
    expect(status.gitlab.signedIn).toBe(false);
    expect(status.linear.hasKey).toBe(false);
  });

  test('D-04 agents that are not installed cannot be picked, so only the practice agent runs', async ({ app }) => {
    await app.openHome();
    const agents = app.page.getByLabel('Agent', { exact: true });
    await expect(agents.getByRole('option', { name: 'Practice agent' })).toBeEnabled();
    for (const name of ['Claude', 'Codex', 'Gemini']) {
      await expect(agents.getByRole('option', { name: `${name} (not installed)` })).toBeDisabled();
    }
    await expect(agents).toHaveValue('mock');

    const listed = await app.core.request<{ id: string; available: boolean; note: string | null }[]>('agents.list');
    expect(listed.filter((agent) => agent.available).map((agent) => agent.id)).toEqual(['mock']);
    expect(listed.find((agent) => agent.id === 'claude')?.note).toContain('npx command was not found');
  });

  test('Q-02 the core answers on 127.0.0.1 only', async ({ core }) => {
    const others = Object.values(networkInterfaces())
      .flat()
      .filter((address) => address && address.family === 'IPv4' && !address.internal)
      .map((address) => address!.address);
    test.skip(others.length === 0, 'this machine has no other address to try');
    await expect((await fetch(`${core.url}/api/health`)).json()).resolves.toMatchObject({ ok: true });
    for (const address of others) {
      await expect(
        fetch(`http://${address}:${core.port}/api/health`, { signal: AbortSignal.timeout(3000) }),
      ).rejects.toThrow();
    }
  });
});
