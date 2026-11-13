/** A conversation of Claude Code as the core lists it. */
export type ClaudeSession = {
  /** The name Claude Code gave the session. */
  id: string;
  /** The first thing the user said in it. */
  title: string;
  startedAt: number;
  lastAt: number;
  /** The folder it worked in, and whether that folder is still there. */
  cwd: string;
  folderThere: boolean;
  /** The branch it ended on. */
  branch: string | null;
  /** The task it was brought in as, when it was. */
  taskId: string | null;
};

/** The conversations whose first words, branch or folder hold every word that was typed, in any case. */
export function matching(sessions: ClaudeSession[], typed: string): ClaudeSession[] {
  const words = typed.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return sessions;
  return sessions.filter((one) => {
    const where = `${one.title}\n${one.branch ?? ''}\n${one.cwd}`.toLowerCase();
    return words.every((word) => where.includes(word));
  });
}

/** Where a conversation worked, told from the project's folder: "the project's folder", or the way down from it. */
export function placeOf(session: ClaudeSession, projectPath: string): string {
  if (session.cwd === projectPath) return "the project's folder";
  const start = projectPath.endsWith('/') ? projectPath : `${projectPath}/`;
  return session.cwd.startsWith(start) ? session.cwd.slice(start.length) : session.cwd;
}
