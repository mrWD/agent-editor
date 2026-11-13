/**
 * Which icon a file or a folder gets in the tree.
 *
 * The shape says what kind of thing it is — code, data, words, a picture —
 * and the colour says which language or format. Nothing here draws: this is
 * only the reading of a name, so it can be tested as it is.
 */

/** The shape drawn for an entry. */
export type IconShape =
  'folder' | 'folderOpen' | 'code' | 'data' | 'words' | 'image' | 'lock' | 'git' | 'shell' | 'file';

/** What an entry is, as the tree shows it: a shape, a colour and a word for the tooltip. */
export type FileIcon = { shape: IconShape; tone: string; what: string };

const icon = (shape: IconShape, tone: string, what: string): FileIcon => ({ shape, tone, what });

/** Whole names that say more than their ending does. */
const BY_NAME: Record<string, FileIcon> = {
  'package.json': icon('data', 'npm', 'npm package'),
  'package-lock.json': icon('lock', 'muted', 'npm lock file'),
  'pnpm-lock.yaml': icon('lock', 'muted', 'pnpm lock file'),
  'yarn.lock': icon('lock', 'muted', 'Yarn lock file'),
  'bun.lockb': icon('lock', 'muted', 'Bun lock file'),
  'cargo.toml': icon('data', 'rust', 'Cargo package'),
  'cargo.lock': icon('lock', 'muted', 'Cargo lock file'),
  'go.mod': icon('data', 'go', 'Go module'),
  'go.sum': icon('lock', 'muted', 'Go checksums'),
  'composer.lock': icon('lock', 'muted', 'Composer lock file'),
  'poetry.lock': icon('lock', 'muted', 'Poetry lock file'),
  'gemfile.lock': icon('lock', 'muted', 'Bundler lock file'),
  dockerfile: icon('data', 'docker', 'Dockerfile'),
  makefile: icon('shell', 'shell', 'Makefile'),
  justfile: icon('shell', 'shell', 'Justfile'),
  license: icon('words', 'muted', 'Licence'),
  readme: icon('words', 'words', 'Readme'),
};

/** Endings, lower case and without the dot. */
const BY_ENDING: Record<string, FileIcon> = {
  ts: icon('code', 'ts', 'TypeScript'),
  tsx: icon('code', 'ts', 'TypeScript, with markup'),
  mts: icon('code', 'ts', 'TypeScript'),
  cts: icon('code', 'ts', 'TypeScript'),
  js: icon('code', 'js', 'JavaScript'),
  jsx: icon('code', 'js', 'JavaScript, with markup'),
  mjs: icon('code', 'js', 'JavaScript'),
  cjs: icon('code', 'js', 'JavaScript'),
  rs: icon('code', 'rust', 'Rust'),
  py: icon('code', 'python', 'Python'),
  go: icon('code', 'go', 'Go'),
  rb: icon('code', 'ruby', 'Ruby'),
  php: icon('code', 'php', 'PHP'),
  java: icon('code', 'java', 'Java'),
  kt: icon('code', 'java', 'Kotlin'),
  swift: icon('code', 'swift', 'Swift'),
  c: icon('code', 'c', 'C'),
  h: icon('code', 'c', 'C header'),
  cpp: icon('code', 'c', 'C++'),
  hpp: icon('code', 'c', 'C++ header'),
  cs: icon('code', 'csharp', 'C#'),
  vue: icon('code', 'vue', 'Vue component'),
  svelte: icon('code', 'svelte', 'Svelte component'),
  html: icon('code', 'html', 'HTML'),
  htm: icon('code', 'html', 'HTML'),
  css: icon('code', 'css', 'CSS'),
  scss: icon('code', 'css', 'Sass'),
  sass: icon('code', 'css', 'Sass'),
  less: icon('code', 'css', 'Less'),
  sql: icon('data', 'sql', 'SQL'),
  json: icon('data', 'data', 'JSON'),
  jsonc: icon('data', 'data', 'JSON with comments'),
  yaml: icon('data', 'data', 'YAML'),
  yml: icon('data', 'data', 'YAML'),
  toml: icon('data', 'data', 'TOML'),
  ini: icon('data', 'data', 'Settings'),
  env: icon('data', 'env', 'Environment'),
  xml: icon('data', 'data', 'XML'),
  csv: icon('data', 'data', 'Table'),
  md: icon('words', 'words', 'Markdown'),
  mdx: icon('words', 'words', 'Markdown, with components'),
  txt: icon('words', 'muted', 'Text'),
  rst: icon('words', 'muted', 'Text'),
  adoc: icon('words', 'muted', 'Text'),
  sh: icon('shell', 'shell', 'Shell script'),
  bash: icon('shell', 'shell', 'Shell script'),
  zsh: icon('shell', 'shell', 'Shell script'),
  fish: icon('shell', 'shell', 'Shell script'),
  ps1: icon('shell', 'shell', 'PowerShell script'),
  bat: icon('shell', 'shell', 'Batch file'),
  cmd: icon('shell', 'shell', 'Batch file'),
  png: icon('image', 'image', 'Picture'),
  jpg: icon('image', 'image', 'Picture'),
  jpeg: icon('image', 'image', 'Picture'),
  gif: icon('image', 'image', 'Picture'),
  webp: icon('image', 'image', 'Picture'),
  avif: icon('image', 'image', 'Picture'),
  bmp: icon('image', 'image', 'Picture'),
  ico: icon('image', 'image', 'Icon'),
  icns: icon('image', 'image', 'Icon'),
  svg: icon('image', 'svg', 'Drawing'),
  pdf: icon('words', 'pdf', 'PDF'),
  lock: icon('lock', 'muted', 'Lock file'),
  log: icon('words', 'muted', 'Log'),
};

/** The name without its folders, lower case. */
function baseName(path: string): string {
  const cut = path.replace(/[/\\]+$/, '');
  const slash = Math.max(cut.lastIndexOf('/'), cut.lastIndexOf('\\'));
  return (slash < 0 ? cut : cut.slice(slash + 1)).toLowerCase();
}

/** The ending of a name, lower case and without the dot. A name that is all ending ("`.gitignore`") has none. */
export function endingOf(name: string): string {
  const dot = name.lastIndexOf('.');
  return dot > 0 ? name.slice(dot + 1) : '';
}

/**
 * The icon for one entry of the tree. `open` is for a folder whose files are shown.
 */
export function iconOf(path: string, isDir: boolean, open = false): FileIcon {
  if (isDir) {
    const name = baseName(path);
    const what = open ? 'Folder, open' : 'Folder';
    if (name === '.git') return icon('git', 'git', 'Git');
    return open ? icon('folderOpen', 'folder', what) : icon('folder', 'folder', what);
  }
  const name = baseName(path);
  // A name of its own comes first: `package.json` is an npm package before it is JSON.
  const whole = BY_NAME[name] ?? BY_NAME[name.replace(/\.[^.]+$/, '')];
  if (whole) return whole;
  // Names that are nothing but a dot and a word: `.gitignore`, `.npmrc`, `.env`.
  if (name.startsWith('.')) {
    const word = name.slice(1);
    if (word.startsWith('git')) return icon('git', 'git', 'Git');
    if (word.startsWith('env')) return icon('data', 'env', 'Environment');
    if (!word.includes('.')) return icon('data', 'muted', 'Settings');
  }
  // A lock file of any kind: "something.lock", "something-lock.json".
  if (name.endsWith('.lock') || name.includes('-lock.') || name.includes('.lock.')) {
    return icon('lock', 'muted', 'Lock file');
  }
  const ending = endingOf(name);
  // A test file is still its language; the tree says so in the tooltip.
  const known = BY_ENDING[ending];
  if (known) return known;
  return icon('file', 'muted', ending ? `${ending.toUpperCase()} file` : 'File');
}
