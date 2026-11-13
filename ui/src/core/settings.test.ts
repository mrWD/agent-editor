import { describe, expect, it } from 'vitest';
import { commandLine, envText, joinArgs, parseEnv, shortFor, splitArgs } from './settings';

describe('the arguments of an agent, typed on one line', () => {
  it('are parted by spaces, however many', () => {
    expect(splitArgs('-y @agentclientprotocol/claude-agent-acp')).toEqual([
      '-y',
      '@agentclientprotocol/claude-agent-acp',
    ]);
    expect(splitArgs('  --acp \t --model   fast ')).toEqual(['--acp', '--model', 'fast']);
    expect(splitArgs('')).toEqual([]);
    expect(splitArgs('   ')).toEqual([]);
  });

  it('are kept together by quotes of either kind', () => {
    expect(splitArgs('--name "My agent" --note \'a "b" c\'')).toEqual(['--name', 'My agent', '--note', 'a "b" c']);
    expect(splitArgs('--flag="two words"')).toEqual(['--flag=two words']);
    // An empty pair of quotes is an argument too.
    expect(splitArgs('--prefix "" last')).toEqual(['--prefix', '', 'last']);
    // A quote that is never closed takes the rest of the line.
    expect(splitArgs('say "hello there')).toEqual(['say', 'hello there']);
  });

  it('take a character after a backslash as it is', () => {
    expect(splitArgs('a\\ b c')).toEqual(['a b', 'c']);
    expect(splitArgs('"say \\"hi\\""')).toEqual(['say "hi"']);
    // Inside single quotes a backslash is only a backslash.
    expect(splitArgs("'C:\\tools\\agent'")).toEqual(['C:\\tools\\agent']);
    // Nothing is expanded: this is not a shell.
    expect(splitArgs('$HOME/agent ~/x *.js')).toEqual(['$HOME/agent', '~/x', '*.js']);
  });

  it('are written back so that reading them gives the same again', () => {
    for (const args of [
      [],
      ['-y', '@google/gemini-cli', '--acp'],
      ['--name', 'My agent'],
      ['', 'a "quoted" word', "it's", 'C:\\tools\\agent', 'tab\there'],
    ]) {
      expect(splitArgs(joinArgs(args))).toEqual(args);
    }
    expect(joinArgs(['-y', 'pkg'])).toBe('-y pkg');
    expect(joinArgs(['--name', 'My agent'])).toBe('--name "My agent"');
  });

  it('follow the program on the line that shows the whole command', () => {
    expect(commandLine({ program: 'npx', args: ['-y', '@google/gemini-cli', '--acp'] })).toBe(
      'npx -y @google/gemini-cli --acp',
    );
    expect(commandLine({ program: '/Applications/My Agent/agent', args: [] })).toBe('"/Applications/My Agent/agent"');
  });
});

describe('the environment of an agent, typed one variable on a line', () => {
  it('is read as names and values; the value is everything after the first "="', () => {
    expect(parseEnv('API_KEY=abc=def\n\n  REGION = eu \n')).toEqual({
      env: [
        ['API_KEY', 'abc=def'],
        ['REGION', ' eu '],
      ],
      problem: null,
    });
    expect(parseEnv('')).toEqual({ env: [], problem: null });
    expect(parseEnv('EMPTY=')).toEqual({ env: [['EMPTY', '']], problem: null });
  });

  it('says which line is not a variable', () => {
    expect(parseEnv('API_KEY=abc\njust words').problem).toBe(
      '"just words" is not NAME=value. Write one variable on a line.',
    );
    expect(parseEnv('=value').problem).toContain('is not NAME=value');
    expect(parseEnv('TWO WORDS=1').problem).toContain('is not NAME=value');
  });

  it('is written back as it was read', () => {
    const env: [string, string][] = [
      ['A', '1'],
      ['B', 'x=y z'],
    ];
    expect(envText(env)).toBe('A=1\nB=x=y z');
    expect(parseEnv(envText(env)).env).toEqual(env);
  });
});

describe('the mark of an agent', () => {
  it('is made from the first two letters of its name', () => {
    expect(shortFor('Claude')).toBe('Cl');
    expect(shortFor('my agent')).toBe('My');
    expect(shortFor('  X ')).toBe('X');
    expect(shortFor('--')).toBe('');
  });
});
