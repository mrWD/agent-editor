import { describe, expect, it } from 'vitest';
import { MAX_BYTES, MAX_PICTURES, refusal, sizeWords } from './pictures';

const file = (name: string, type: string, size: number) => ({ name, type, size });

describe('pictures in a message', () => {
  it('takes the kinds agents take, up to a size and a count', () => {
    expect(refusal(file('shot.png', 'image/png', 2000), 0)).toBeNull();
    expect(refusal(file('photo.jpg', 'image/jpeg', MAX_BYTES), MAX_PICTURES - 1)).toBeNull();
    expect(refusal(file('page.svg', 'image/svg+xml', 2000), 0)).toBe(
      'page.svg is not a picture an agent can take. PNG, JPEG, GIF and WebP are.',
    );
    expect(refusal(file('notes.txt', 'text/plain', 20), 0)).toContain('is not a picture');
    expect(refusal(file('big.png', 'image/png', MAX_BYTES + 1), 0)).toBe('big.png is larger than 5 MB.');
    expect(refusal(file('none.png', 'image/png', 0), 0)).toBe('none.png is empty.');
    expect(refusal(file('sixth.png', 'image/png', 2000), MAX_PICTURES)).toBe('A message takes 5 pictures at most.');
  });

  it('says a size in words', () => {
    expect(sizeWords(70)).toBe('70 bytes');
    expect(sizeWords(12 * 1024)).toBe('12 KB');
    expect(sizeWords(1.44 * 1024 * 1024)).toBe('1.4 MB');
  });
});
