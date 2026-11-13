import { createStore } from 'solid-js/store';
import { For, Show, type JSX } from 'solid-js';
import { KINDS, refusal, sizeWords } from '../core/pictures';
import { pictureUrl, type PictureName, type PictureToSend } from '../core/store';

/** A picture waiting in the message box: what will be sent, and an address to show it by until then. */
type Waiting = PictureToSend & { shown: string; size: number };

// The pictures of each task's unsent message. They live outside the pane, so they survive a look at another task.
const [waiting, setWaiting] = createStore<Record<string, { list: Waiting[]; problem: string | null }>>({});
const of = (taskId: string) => waiting[taskId] ?? { list: [], problem: null };

export const pictureCount = (taskId: string) => of(taskId).list.length;

const read = (file: File) =>
  new Promise<string>((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.onerror = () => reject(reader.error ?? new Error(`${file.name} could not be read.`));
    reader.readAsDataURL(file);
  });

/** Puts files into the unsent message of a task. What cannot go is said next to the pictures. */
export async function addPictures(taskId: string, files: File[]): Promise<void> {
  let problem: string | null = null;
  for (const file of files) {
    const why = refusal(file, of(taskId).list.length);
    if (why) {
      problem = why;
      continue;
    }
    try {
      const shown = await read(file);
      const data = shown.slice(shown.indexOf(',') + 1);
      const one: Waiting = { name: file.name || 'picture', mimeType: file.type, data, shown, size: file.size };
      setWaiting(taskId, { list: [...of(taskId).list, one], problem: of(taskId).problem });
    } catch (error) {
      problem = error instanceof Error ? error.message : String(error);
    }
  }
  setWaiting(taskId, { list: of(taskId).list, problem });
}

/** Takes the pictures out of the message box to send them. `putBack` returns them if sending failed. */
export function takePictures(taskId: string): Waiting[] {
  const list = of(taskId).list;
  if (list.length > 0 || of(taskId).problem) setWaiting(taskId, { list: [], problem: null });
  return list;
}
export function putBack(taskId: string, list: PictureToSend[]) {
  if (list.length > 0) setWaiting(taskId, { list: list as Waiting[], problem: null });
}

/** The image files of a paste or a drop. Text and everything else is left to the box itself. */
function imagesIn(data: DataTransfer | null): File[] {
  return [...(data?.files ?? [])].filter((file) => file.type.startsWith('image/'));
}

/** For the message box: pictures pasted into it go into the message. */
export const pastePictures = (taskId: () => string, allowed: () => boolean) => (event: ClipboardEvent) => {
  const files = imagesIn(event.clipboardData);
  if (files.length === 0 || !allowed()) return;
  event.preventDefault();
  void addPictures(taskId(), files);
};

/** For the message box: pictures dropped onto it go into the message. */
export const dropPictures = (taskId: () => string, allowed: () => boolean) => (event: DragEvent) => {
  const files = imagesIn(event.dataTransfer);
  if (files.length === 0) return;
  event.preventDefault();
  if (allowed()) void addPictures(taskId(), files);
};

/** The pictures waiting to be sent, above the message box, each with a way to leave it out. */
export function WaitingPictures(props: { taskId: string }): JSX.Element {
  const mine = () => of(props.taskId);
  const remove = (index: number) =>
    setWaiting(props.taskId, { list: mine().list.filter((_, at) => at !== index), problem: null });
  return (
    <>
      <Show when={mine().list.length > 0}>
        <ul class="pictures waiting" aria-label="Pictures that go with your message">
          <For each={mine().list}>
            {(picture, index) => (
              <li>
                <img src={picture.shown} alt="" />
                <span class="name">{picture.name}</span>
                <span class="size">{sizeWords(picture.size)}</span>
                <button type="button" aria-label={`Leave ${picture.name} out`} onClick={() => remove(index())}>
                  ×
                </button>
              </li>
            )}
          </For>
        </ul>
      </Show>
      <Show when={mine().problem}>
        <p class="notice danger" role="alert">
          {mine().problem}
        </p>
      </Show>
    </>
  );
}

/** The button that opens the file dialog. Off, with the reason, for an agent that takes no pictures. */
export function AttachPictures(props: { taskId: string; allowed: boolean; agentName: string }): JSX.Element {
  let input: HTMLInputElement | undefined;
  return (
    <>
      <input
        ref={input}
        type="file"
        class="sr-only"
        accept={KINDS.join(',')}
        multiple
        tabindex="-1"
        aria-label="Pictures to add to your message"
        onChange={(event) => {
          void addPictures(props.taskId, [...(event.currentTarget.files ?? [])]);
          event.currentTarget.value = '';
        }}
      />
      <button
        type="button"
        class="btn ghost attach"
        disabled={!props.allowed}
        title={
          props.allowed
            ? 'Add a picture: a screenshot, a mockup. You can also paste one into the message.'
            : `${props.agentName} does not take pictures`
        }
        onClick={() => input?.click()}
      >
        <svg
          width="14"
          height="14"
          viewBox="0 0 16 16"
          fill="none"
          stroke="currentColor"
          stroke-width="1.4"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          <rect x="2" y="3" width="12" height="10" rx="1.5" />
          <circle cx="5.5" cy="6.5" r="1" />
          <path d="M2.5 11.5l3.5-3 2.5 2 2-1.5 3 2.5" />
        </svg>
        Picture
      </button>
    </>
  );
}

/** The pictures of a message that was sent, in the conversation. Each opens whole in a new tab. */
export function MessagePictures(props: { taskId: string; images: PictureName[] }): JSX.Element {
  return (
    <Show when={props.images.length > 0}>
      <ul class="pictures sent" aria-label="Pictures sent with this message">
        <For each={props.images}>
          {(picture) => (
            <li>
              <a href={pictureUrl(props.taskId, picture)} target="_blank" rel="noreferrer" title={picture.name}>
                <img src={pictureUrl(props.taskId, picture)} alt={picture.name} loading="lazy" />
              </a>
            </li>
          )}
        </For>
      </ul>
    </Show>
  );
}
