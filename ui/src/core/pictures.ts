/**
 * What can go into a message as a picture, worked out before anything is sent:
 * the core checks the same again. No drawing here, so it can be tested as it is.
 */

/** What the core takes: how many pictures in one message, how large each, and of which kinds. */
export const MAX_PICTURES = 5;
export const MAX_BYTES = 5 * 1024 * 1024;
export const KINDS = ['image/png', 'image/jpeg', 'image/gif', 'image/webp'];

/** Why a file cannot go into a message, or nothing when it can. */
export function refusal(file: { name: string; type: string; size: number }, already: number): string | null {
  if (already >= MAX_PICTURES) return `A message takes ${MAX_PICTURES} pictures at most.`;
  if (!KINDS.includes(file.type))
    return `${file.name} is not a picture an agent can take. PNG, JPEG, GIF and WebP are.`;
  if (file.size > MAX_BYTES) return `${file.name} is larger than ${MAX_BYTES / 1024 / 1024} MB.`;
  if (file.size === 0) return `${file.name} is empty.`;
  return null;
}

/** "68 bytes", "12 KB", "1.4 MB". */
export function sizeWords(bytes: number): string {
  if (bytes < 1024) return `${bytes} bytes`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}
