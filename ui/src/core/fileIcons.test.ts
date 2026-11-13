import { describe, expect, it } from 'vitest';
import { endingOf, iconOf } from './fileIcons';

const shape = (path: string, isDir = false, open = false) => iconOf(path, isDir, open).shape;
const tone = (path: string) => iconOf(path, false).tone;

describe('the icon of an entry in the tree', () => {
  it('draws a folder, open or closed', () => {
    expect(shape('src', true)).toBe('folder');
    expect(shape('src', true, true)).toBe('folderOpen');
    expect(iconOf('src/middleware', true, true).what).toBe('Folder, open');
    // Git's own folder is git, open or not.
    expect(shape('.git', true)).toBe('git');
    expect(shape('.git', true, true)).toBe('git');
  });

  it('tells code from data, words, pictures and scripts', () => {
    expect(shape('src/app.ts')).toBe('code');
    expect(shape('src/app.rs')).toBe('code');
    expect(shape('tsconfig.json')).toBe('data');
    expect(shape('README.md')).toBe('words');
    expect(shape('design/logo.png')).toBe('image');
    expect(shape('scripts/sonar-local.sh')).toBe('shell');
    expect(shape('Makefile')).toBe('shell');
    expect(shape('src/anything.unknown')).toBe('file');
  });

  it('gives each language its own colour', () => {
    expect(tone('a.ts')).toBe('ts');
    expect(tone('a.tsx')).toBe('ts');
    expect(tone('a.mjs')).toBe('js');
    expect(tone('a.rs')).toBe('rust');
    expect(tone('a.py')).toBe('python');
    expect(tone('a.go')).toBe('go');
    expect(tone('a.css')).toBe('css');
    expect(tone('a.svg')).toBe('svg');
    expect(tone('a.unknown')).toBe('muted');
  });

  it('knows names that say more than their ending', () => {
    expect(iconOf('package.json', false)).toMatchObject({ shape: 'data', tone: 'npm', what: 'npm package' });
    expect(iconOf('Cargo.toml', false)).toMatchObject({ tone: 'rust', what: 'Cargo package' });
    expect(iconOf('Dockerfile', false)).toMatchObject({ tone: 'docker' });
    expect(iconOf('README.md', false).what).toBe('Readme');
    expect(iconOf('LICENSE', false).what).toBe('Licence');
  });

  it('marks every kind of lock file', () => {
    for (const name of ['package-lock.json', 'Cargo.lock', 'yarn.lock', 'pnpm-lock.yaml', 'poetry.lock', 'a.lock']) {
      expect(shape(name), name).toBe('lock');
    }
    // A name that only looks like one is not.
    expect(shape('src/locked.ts')).toBe('code');
  });

  it('reads names that are nothing but a dot and a word', () => {
    expect(iconOf('.gitignore', false)).toMatchObject({ shape: 'git', tone: 'git' });
    expect(iconOf('.gitattributes', false).shape).toBe('git');
    expect(iconOf('.env', false)).toMatchObject({ shape: 'data', tone: 'env' });
    expect(iconOf('.env.local', false)).toMatchObject({ shape: 'data', tone: 'env' });
    expect(iconOf('.npmrc', false)).toMatchObject({ shape: 'data', what: 'Settings' });
  });

  it('reads the name out of a path, whichever way it is written', () => {
    expect(tone('src/middleware/rateLimit.ts')).toBe('ts');
    expect(tone('C:\\work\\shop\\src\\app.rs')).toBe('rust');
    expect(shape('src/', true)).toBe('folder');
    expect(endingOf('rateLimit.ts')).toBe('ts');
    expect(endingOf('.gitignore')).toBe('');
    expect(endingOf('Makefile')).toBe('');
  });

  it('says what an unknown ending is, for the tooltip', () => {
    expect(iconOf('notes.wat', false).what).toBe('WAT file');
    expect(iconOf('Procfile', false).what).toBe('File');
  });
});
