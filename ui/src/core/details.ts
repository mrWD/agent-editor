/**
 * An issue or a merge request in full, as the core reads it from the tracker,
 * and the words the side panel makes of it. Nothing here asks the core.
 */

export type DetailNote = {
  author: string;
  at: number;
  /** What it says, in Markdown. */
  body: string;
  kind: 'comment' | 'review';
  /** For a review: "approved", "asked for changes". For a comment on a line: the file. Else empty. */
  about: string;
};

export type Detail = {
  source: 'github' | 'gitlab' | 'linear';
  /** How people call it: `#12`, `!48`, `APP-214`. */
  key: string;
  title: string;
  url: string;
  state: string;
  draft: boolean;
  author: string;
  assignees: string[];
  reviewers: string[];
  labels: string[];
  createdAt: number;
  updatedAt: number;
  /** The description, in Markdown. */
  body: string;
  links: { title: string; url: string }[];
  /** What was written under it, oldest first. */
  notes: DetailNote[];
  /** For a request: the branch it comes from and the one it goes into. */
  branch: string | null;
  base: string | null;
};

export const trackerName: Record<Detail['source'], string> = { github: 'GitHub', gitlab: 'GitLab', linear: 'Linear' };

/**
 * The address a picture of a tracker is shown from: the core fetches it with the user's own
 * sign-in, since the tracker gives it to nobody else. Whether the address is the tracker's own is
 * the core's to judge; one that is not comes back as no picture.
 */
export function pictureFrom(projectId: string): (src: string) => string {
  return (src) => `/api/tracker/picture?project=${encodeURIComponent(projectId)}&src=${encodeURIComponent(src)}`;
}

/** "Ann and Bob", "Ann, Bob and Cy": names as a person lists them. */
export function listed(names: string[]): string {
  if (names.length <= 1) return names[0] ?? '';
  return `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}`;
}

/** Who is on it, a line for each kind: who wrote it, who has it, who was asked to review. */
export function peopleLines(detail: Pick<Detail, 'author' | 'assignees' | 'reviewers'>): string[] {
  const lines: string[] = [];
  if (detail.author) lines.push(`Opened by ${detail.author}`);
  if (detail.assignees.length > 0) lines.push(`Assigned to ${listed(detail.assignees)}`);
  if (detail.reviewers.length > 0) lines.push(`Review asked of ${listed(detail.reviewers)}`);
  return lines;
}

/** "3 comments", "1 comment and 2 reviews": what was written under it, counted. */
export function notesWords(notes: DetailNote[]): string {
  const comments = notes.filter((note) => note.kind === 'comment').length;
  const reviews = notes.length - comments;
  const count = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;
  const parts = [
    comments > 0 ? count(comments, 'comment', 'comments') : '',
    reviews > 0 ? count(reviews, 'review', 'reviews') : '',
  ];
  return parts.filter(Boolean).join(' and ') || 'Nobody has written under it';
}

/** What stands beside the author of a note: what a reviewer decided, or the file a comment is about. */
export function noteAbout(note: Pick<DetailNote, 'kind' | 'about'>): string {
  if (note.about === '') return '';
  return note.kind === 'review' ? note.about : `on ${note.about}`;
}
