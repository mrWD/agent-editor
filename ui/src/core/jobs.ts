/**
 * The Jobs view of the Preview: what the core says about the background jobs
 * of a project, and the words made of it. Nothing here asks the core.
 */

export type JobKind = 'celery' | 'dramatiq' | 'django' | 'artisan' | 'sidekiq' | 'activejob' | 'script';

export type Job = {
  /** Its kind and its name, such as `celery:shop.tasks.send_receipt`. */
  id: string;
  kind: JobKind;
  name: string;
  file: string;
  line: number;
  /** What it takes, as written: `order_id`, `force=False`. */
  takes: string[];
  /** Whether the task touched its file. */
  changed: boolean;
  folder: string;
};

export type JobsState = {
  jobs: Job[];
  celeryApp: string | null;
  /** The command line of each kind as it is unless the project says otherwise. */
  templates: Record<string, string>;
  /** The project's own command line of each kind. Empty when it has none. */
  settings: Record<string, string>;
};

/** How the last run of a job stands. */
export type JobRun = {
  running: boolean;
  termId: number | null;
  /** The code it ended with. `null` while it runs, and when it never ran. */
  exit: number | null;
  /** The end of what it printed. */
  output: string | null;
  /** Set here, not by the core: the user stopped it, and this is what it had printed until then. */
  stopped?: boolean;
};

export const kindWord: Record<JobKind, string> = {
  celery: 'Celery task',
  dramatiq: 'Dramatiq actor',
  django: 'Django command',
  artisan: 'Laravel command',
  sidekiq: 'Sidekiq worker',
  activejob: 'Active Job',
  script: 'Package script',
};

/** Whether a job is put on a queue, with JSON for its arguments, and not run as a command. */
export function isQueued(kind: JobKind): boolean {
  return kind === 'celery' || kind === 'dramatiq' || kind === 'sidekiq' || kind === 'activejob';
}

/** Whether a queued job takes arguments by name too. Ruby's take a list only. */
export function takesNamed(kind: JobKind): boolean {
  return kind === 'celery' || kind === 'dramatiq';
}

/** What to show in the empty box for arguments: the names the job takes, in the shape they are typed in. */
export function argsHint(job: Pick<Job, 'kind' | 'takes'>): string {
  if (!isQueued(job.kind))
    return job.takes.length > 0 ? job.takes.join(' ') : 'what comes after the command, as you would type it';
  const needed = job.takes.filter((one) => !one.includes('=') && !one.startsWith('*'));
  return `[${needed.join(', ')}]`;
}

/** Why what was typed cannot be sent to a queue, or `null`. The core says the same; this says it sooner. */
export function argsProblem(kind: JobKind, args: string, kwargs: string): string | null {
  if (!isQueued(kind)) return null;
  const parse = (text: string, what: string, fits: (value: unknown) => boolean, shape: string) => {
    if (text.trim() === '') return null;
    try {
      return fits(JSON.parse(text)) ? null : `${what} are ${shape}`;
    } catch {
      return `${what} are not JSON yet`;
    }
  };
  return (
    parse(args, 'The arguments', Array.isArray, 'a JSON list, such as [42, "tea"]') ??
    parse(
      kwargs,
      'The named arguments',
      (value) => typeof value === 'object' && value !== null && !Array.isArray(value),
      'a JSON object, such as {"force": true}',
    )
  );
}

/** How a run stands, in a few words, and whether that is good. */
export function runWords(run: JobRun | null): { text: string; tone: 'good' | 'bad' | 'plain' } | null {
  if (!run) return null;
  if (run.running) return { text: 'Running', tone: 'plain' };
  if (run.stopped) return { text: 'Stopped', tone: 'plain' };
  if (run.exit === null) return null;
  return run.exit === 0 ? { text: 'Ended well', tone: 'good' } : { text: `Ended with code ${run.exit}`, tone: 'bad' };
}

/** A service of the task that does the queued jobs, by what it is called or what it runs. */
export function workerOf<T extends { name: string; runs: string; state: string }>(services: T[]): T | null {
  const does = (service: T) => /celery|worker|sidekiq|dramatiq|queue|\brq\b/i.test(`${service.name} ${service.runs}`);
  const workers = services.filter(does);
  return workers.find((service) => service.state === 'running') ?? workers[0] ?? null;
}

/** What to say about who will do a queued job. */
export function workerWords(worker: { name: string; state: string } | null): string {
  if (!worker)
    return 'No worker is among what this task runs. A job put on a queue waits until a worker of the project takes it.';
  if (worker.state === 'running') return `The worker ${worker.name} is running: what the job prints there is in Run.`;
  return `The worker ${worker.name} is not running. Start it in Run, or the job waits on the queue.`;
}

/** The jobs that match what was typed into the filter. */
export function matchingJobs(jobs: Job[], filter: string): Job[] {
  const words = filter.toLowerCase().split(/\s+/).filter(Boolean);
  if (words.length === 0) return jobs;
  return jobs.filter((job) =>
    words.every((word) => `${job.name} ${kindWord[job.kind]} ${job.file}`.toLowerCase().includes(word)),
  );
}

/** A run as text for the agent: what was started, how it ended, what it printed. */
export function runForAgent(job: Job, command: string, run: JobRun): { title: string; text: string } {
  const ended = run.running
    ? 'It is still running.'
    : run.stopped
      ? 'It was stopped by hand.'
      : run.exit === null
        ? ''
        : run.exit === 0
          ? 'It ended well.'
          : `It ended with code ${run.exit}.`;
  const title = `${job.name} ${run.running ? 'is running' : run.stopped ? 'was stopped' : run.exit === 0 ? 'ran' : 'failed'}`;
  return {
    title,
    text: [
      `${kindWord[job.kind]} ${job.name} (${job.file}:${job.line})`,
      `$ ${command}`,
      '',
      run.output?.trim() || '(it printed nothing)',
      '',
      ended,
    ]
      .join('\n')
      .trimEnd(),
  };
}
