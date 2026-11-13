import { describe, expect, it } from 'vitest';
import { readChoice, resolve } from './theme';

describe('the theme', () => {
  it('is dark unless something else was chosen: dark is the design', () => {
    expect(readChoice(null)).toBe('dark');
    expect(readChoice('')).toBe('dark');
    expect(readChoice('sepia')).toBe('dark');
    expect(readChoice('dark')).toBe('dark');
    expect(readChoice('light')).toBe('light');
    expect(readChoice('system')).toBe('system');
  });

  it('follows the computer only when asked to', () => {
    expect(resolve('system', true)).toBe('light');
    expect(resolve('system', false)).toBe('dark');
    expect(resolve('dark', true)).toBe('dark');
    expect(resolve('light', false)).toBe('light');
  });
});
