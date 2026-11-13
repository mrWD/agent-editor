import { describe, expect, it } from 'vitest';
import { chooseFolder, shellMark } from './shell';

describe('the mark of the desktop app', () => {
  it('is not there in a browser', () => {
    expect(shellMark({})).toBeNull();
    expect(shellMark(undefined)).toBeNull();
    // The tests themselves run outside the app.
    expect(shellMark()).toBeNull();
  });

  it('is read from the window the app opened', () => {
    expect(shellMark({ __AE_SHELL__: { key: 'abc' } })).toEqual({ key: 'abc' });
  });

  it('counts only when it carries a key', () => {
    for (const broken of [null, true, 'abc', {}, { key: '' }, { key: 7 }]) {
      expect(shellMark({ __AE_SHELL__: broken })).toBeNull();
    }
  });
});

describe('choosing a folder', () => {
  /** A core that answers the dialog with `path` and notes what it was asked. */
  function coreThatAnswers(path: string | null) {
    const asked: { method: string; params: Record<string, unknown> }[] = [];
    const request = <T>(method: string, params: Record<string, unknown>) => {
      asked.push({ method, params });
      return Promise.resolve({ path } as T);
    };
    return { asked, request };
  }

  it('asks nobody in a browser: the path is typed there', async () => {
    const core = coreThatAnswers('/somewhere');
    expect(await chooseFolder(null, core.request)).toBeNull();
    expect(core.asked).toEqual([]);
  });

  it('gives the folder the user chose in the app, and shows the key of its window', async () => {
    const core = coreThatAnswers('/Users/you/code/shop');
    expect(await chooseFolder({ key: 'abc' }, core.request)).toBe('/Users/you/code/shop');
    expect(core.asked).toEqual([{ method: 'dialogs.pickFolder', params: { key: 'abc' } }]);
  });

  it('gives nothing when the user closed the dialog', async () => {
    expect(await chooseFolder({ key: 'abc' }, coreThatAnswers(null).request)).toBeNull();
    expect(await chooseFolder({ key: 'abc' }, coreThatAnswers('').request)).toBeNull();
  });

  it('passes on what the core says when the dialog could not be shown', async () => {
    const refuses = () => Promise.reject(new Error('Only the desktop app can show a folder dialog.'));
    await expect(chooseFolder({ key: 'abc' }, refuses)).rejects.toThrow('Only the desktop app');
  });
});
