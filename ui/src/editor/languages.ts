import type { Extension } from '@codemirror/state';

/** A language the editor can colour. Its code is fetched the first time a file needs it. */
export type Language = { name: string; load: () => Promise<Extension> };

const legacy = async (pick: () => Promise<any>): Promise<Extension> => {
  const [{ StreamLanguage }, mode] = await Promise.all([import('@codemirror/language'), pick()]);
  return StreamLanguage.define(mode);
};

const javascript = (name: string, options: { typescript?: boolean; jsx?: boolean }): Language => ({
  name,
  load: async () => (await import('@codemirror/lang-javascript')).javascript(options),
});

const byExtension: Record<string, Language> = {
  ts: javascript('TypeScript', { typescript: true }),
  mts: javascript('TypeScript', { typescript: true }),
  cts: javascript('TypeScript', { typescript: true }),
  tsx: javascript('TypeScript JSX', { typescript: true, jsx: true }),
  js: javascript('JavaScript', {}),
  mjs: javascript('JavaScript', {}),
  cjs: javascript('JavaScript', {}),
  jsx: javascript('JavaScript JSX', { jsx: true }),
  json: { name: 'JSON', load: async () => (await import('@codemirror/lang-json')).json() },
  jsonc: { name: 'JSON', load: async () => (await import('@codemirror/lang-json')).json() },
  html: { name: 'HTML', load: async () => (await import('@codemirror/lang-html')).html() },
  htm: { name: 'HTML', load: async () => (await import('@codemirror/lang-html')).html() },
  vue: { name: 'Vue', load: async () => (await import('@codemirror/lang-html')).html() },
  svelte: { name: 'Svelte', load: async () => (await import('@codemirror/lang-html')).html() },
  css: { name: 'CSS', load: async () => (await import('@codemirror/lang-css')).css() },
  scss: { name: 'SCSS', load: () => legacy(async () => (await import('@codemirror/legacy-modes/mode/css')).sCSS) },
  less: { name: 'Less', load: () => legacy(async () => (await import('@codemirror/legacy-modes/mode/css')).less) },
  md: { name: 'Markdown', load: async () => (await import('@codemirror/lang-markdown')).markdown() },
  markdown: { name: 'Markdown', load: async () => (await import('@codemirror/lang-markdown')).markdown() },
  mdx: { name: 'Markdown', load: async () => (await import('@codemirror/lang-markdown')).markdown() },
  py: { name: 'Python', load: async () => (await import('@codemirror/lang-python')).python() },
  rs: { name: 'Rust', load: async () => (await import('@codemirror/lang-rust')).rust() },
  go: { name: 'Go', load: async () => (await import('@codemirror/lang-go')).go() },
  yaml: { name: 'YAML', load: async () => (await import('@codemirror/lang-yaml')).yaml() },
  yml: { name: 'YAML', load: async () => (await import('@codemirror/lang-yaml')).yaml() },
  sql: { name: 'SQL', load: async () => (await import('@codemirror/lang-sql')).sql() },
  xml: { name: 'XML', load: async () => (await import('@codemirror/lang-xml')).xml() },
  svg: { name: 'SVG', load: async () => (await import('@codemirror/lang-xml')).xml() },
  plist: { name: 'XML', load: async () => (await import('@codemirror/lang-xml')).xml() },
  c: { name: 'C', load: async () => (await import('@codemirror/lang-cpp')).cpp() },
  h: { name: 'C', load: async () => (await import('@codemirror/lang-cpp')).cpp() },
  cpp: { name: 'C++', load: async () => (await import('@codemirror/lang-cpp')).cpp() },
  cc: { name: 'C++', load: async () => (await import('@codemirror/lang-cpp')).cpp() },
  hpp: { name: 'C++', load: async () => (await import('@codemirror/lang-cpp')).cpp() },
  java: { name: 'Java', load: async () => (await import('@codemirror/lang-java')).java() },
  php: { name: 'PHP', load: async () => (await import('@codemirror/lang-php')).php() },
  sh: { name: 'Shell', load: () => legacy(async () => (await import('@codemirror/legacy-modes/mode/shell')).shell) },
  bash: { name: 'Shell', load: () => legacy(async () => (await import('@codemirror/legacy-modes/mode/shell')).shell) },
  zsh: { name: 'Shell', load: () => legacy(async () => (await import('@codemirror/legacy-modes/mode/shell')).shell) },
  toml: { name: 'TOML', load: () => legacy(async () => (await import('@codemirror/legacy-modes/mode/toml')).toml) },
  rb: { name: 'Ruby', load: () => legacy(async () => (await import('@codemirror/legacy-modes/mode/ruby')).ruby) },
  swift: { name: 'Swift', load: () => legacy(async () => (await import('@codemirror/legacy-modes/mode/swift')).swift) },
  kt: { name: 'Kotlin', load: () => legacy(async () => (await import('@codemirror/legacy-modes/mode/clike')).kotlin) },
  kts: { name: 'Kotlin', load: () => legacy(async () => (await import('@codemirror/legacy-modes/mode/clike')).kotlin) },
  cs: { name: 'C#', load: () => legacy(async () => (await import('@codemirror/legacy-modes/mode/clike')).csharp) },
  lua: { name: 'Lua', load: () => legacy(async () => (await import('@codemirror/legacy-modes/mode/lua')).lua) },
  diff: { name: 'Diff', load: () => legacy(async () => (await import('@codemirror/legacy-modes/mode/diff')).diff) },
  patch: { name: 'Diff', load: () => legacy(async () => (await import('@codemirror/legacy-modes/mode/diff')).diff) },
  ini: {
    name: 'INI',
    load: () => legacy(async () => (await import('@codemirror/legacy-modes/mode/properties')).properties),
  },
  env: {
    name: 'Env',
    load: () => legacy(async () => (await import('@codemirror/legacy-modes/mode/properties')).properties),
  },
  properties: {
    name: 'Properties',
    load: () => legacy(async () => (await import('@codemirror/legacy-modes/mode/properties')).properties),
  },
};

const byName: Record<string, Language> = {
  dockerfile: {
    name: 'Dockerfile',
    load: () => legacy(async () => (await import('@codemirror/legacy-modes/mode/dockerfile')).dockerFile),
  },
  '.gitignore': byExtension.ini!,
  '.env': byExtension.env!,
  '.zshrc': byExtension.sh!,
  '.bashrc': byExtension.sh!,
};

const plain: Language = { name: 'Plain text', load: async () => [] };

/** The language of a file, judged by its name. */
export function languageOf(path: string): Language {
  const name = (path.split('/').pop() ?? path).toLowerCase();
  if (byName[name]) return byName[name]!;
  if (name.startsWith('dockerfile')) return byName.dockerfile!;
  if (name.startsWith('.env')) return byExtension.env!;
  const dot = name.lastIndexOf('.');
  if (dot < 0) return plain;
  return byExtension[name.slice(dot + 1)] ?? plain;
}
