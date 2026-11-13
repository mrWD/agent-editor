import { createEffect, createSignal, For, Match, on, onCleanup, onMount, Show, Switch, type JSX } from 'solid-js';
import { core } from '../core/client';
import {
  deviceButton,
  deviceLine,
  deviceWords,
  readDevices,
  type Device,
  type DeviceKind,
  type Devices,
} from '../core/preview';

const message = (error: unknown) => (error instanceof Error ? error.message : String(error));
const two = (n: number) => String(n).padStart(2, '0');
/** "14:02:07": the picture changes every second, so the second is told. */
const toTheSecond = (at: number) => {
  const date = new Date(at);
  return `${two(date.getHours())}:${two(date.getMinutes())}:${two(date.getSeconds())}`;
};

/** How often the screen of the device is asked for. */
const EVERY_MS = 1000;
/** A device that gave no picture this many times in a row is no longer on show. One slow answer is not the end of it. */
const GIVES_UP_AFTER = 3;

/** The device a tab shows, as the core said when it opened the address on it. */
type Shown = { id: string; name: string; address: string; startedHere: boolean };

/**
 * A device tab of the Preview: the iOS simulator or the Android emulator.
 *
 * A device cannot be put into a page. What is shown is a picture of its screen,
 * asked of the core again about once a second for as long as the tab is on
 * screen; tapping and typing happen in the device's own window. Leaving the tab
 * tells the core to let the device go.
 */
export function DeviceView(props: {
  taskId: string;
  kind: DeviceKind;
  /** The address of the running app, as typed or as the dev server says. `null` when nothing runs. */
  address: string | null;
  /** Goes up by one each time Reload is pressed. */
  reloads: number;
  /** What to show when no app is running: how to start one. */
  children: JSX.Element;
}): JSX.Element {
  // eslint-disable-next-line solid/reactivity -- the tab is made anew for another task or kind, so neither changes under it
  const { taskId, kind } = props;
  const words = deviceWords(kind);
  const [found, setFound] = createSignal<Devices | null>(null);
  const [shown, setShown] = createSignal<Shown | null>(null);
  const [opening, setOpening] = createSignal<Device | null>(null);
  const [problem, setProblem] = createSignal<string | null>(null);
  const [picture, setPicture] = createSignal<string | null>(null);
  const [takenAt, setTakenAt] = createSignal<number | null>(null);
  /** The device that is shown or being opened, to open the address on it again. */
  let current: Device | null = null;
  /** Whether the core was ever asked to open a device: only then is there something to let go. */
  let asked = false;
  let gone = false;
  let mark: string | null = null;
  /** How many times in a row the device gave no picture. */
  let failures = 0;
  /** A question for the screen is on its way to the core. */
  let asking = false;
  /** The address the picture on show is held under, to give it back when the next one comes. */
  let held: string | null = null;
  let timer: ReturnType<typeof setTimeout> | undefined;

  const look = async () => {
    try {
      setFound(readDevices(await core.request('devices.list', { taskId, kind })));
    } catch (error) {
      setFound({ needs: message(error), devices: [] });
    }
  };

  /** Shows another picture, or none, and gives back the one that was shown. */
  const show = (next: string | null) => {
    const before = held;
    held = next;
    setPicture(next);
    if (before) URL.revokeObjectURL(before);
  };

  const forgetPicture = () => {
    show(null);
    setTakenAt(null);
    mark = null;
  };

  /**
   * Asks for the screen, shows it, and asks again a second after the last asking. Only one asking is ever
   * on its way: called while one is, this does nothing, and the one that is on its way goes on from there.
   */
  const watch = async () => {
    clearTimeout(timer);
    if (gone || !shown() || asking) return;
    // A window nobody looks at needs no pictures. It is looked at again the moment it shows.
    if (document.hidden) {
      timer = setTimeout(() => void watch(), EVERY_MS);
      return;
    }
    const started = Date.now();
    const of = shown()!.id;
    // Whether the answer is still wanted: the tab may be left, or another device picked, while it is on its way.
    const wanted = () => !gone && shown()?.id === of;
    asking = true;
    try {
      const screen = await core.request<{ at: number; mark: string; png?: string }>('devices.screen', {
        taskId,
        have: mark,
      });
      if (wanted()) {
        if (screen.png) {
          const bytes = Uint8Array.from(atob(screen.png), (letter) => letter.charCodeAt(0));
          show(URL.createObjectURL(new Blob([bytes], { type: 'image/png' })));
          mark = screen.mark;
        }
        setTakenAt(screen.at);
        if (failures > 0) setProblem(null);
        failures = 0;
      }
    } catch (error) {
      if (wanted()) {
        // No picture came: the one on show is not known to be its screen any more, so it is taken away.
        failures += 1;
        setProblem(message(error));
        forgetPicture();
        if (failures >= GIVES_UP_AFTER) {
          // The device stopped answering: it is let go, and the list says what is there now.
          await leave();
          void look();
        }
      }
    } finally {
      asking = false;
    }
    if (!gone && shown()) timer = setTimeout(() => void watch(), Math.max(0, EVERY_MS - (Date.now() - started)));
  };

  /** Opens the app's address on a device and starts showing its screen. */
  const open = async (device: Device) => {
    const address = props.address;
    if (!address || opening()) return;
    current = device;
    asked = true;
    failures = 0;
    setOpening(device);
    setProblem(null);
    try {
      const opened = await core.request<Shown>('devices.open', { taskId, kind, id: device.id, address });
      if (gone) return;
      if (shown()?.id !== opened.id) forgetPicture();
      setShown(opened);
      void watch();
    } catch (error) {
      if (gone) return;
      setProblem(message(error));
      // What it could not open it does not show; the list says what is there now.
      if (!shown()) void look();
    } finally {
      setOpening(null);
    }
  };

  /** Lets the device go and comes back to the list. */
  const leave = async () => {
    clearTimeout(timer);
    setShown(null);
    forgetPicture();
    current = null;
    if (asked) await core.request('devices.close', { taskId }).catch(() => {});
  };

  const front = () => {
    setProblem(null);
    core.request('devices.front', { taskId }).catch((error) => setProblem(message(error)));
  };

  onMount(() => {
    void look();
    const stopEvents = core.onEvent((event) => {
      // The core was reached anew: it may be another core, which shows no device.
      if (event.event !== 'link.open') return;
      clearTimeout(timer);
      setShown(null);
      forgetPicture();
      void look();
    });
    const onVisible = () => {
      if (!document.hidden && shown()) void watch();
    };
    document.addEventListener('visibilitychange', onVisible);
    onCleanup(() => {
      gone = true;
      stopEvents();
      document.removeEventListener('visibilitychange', onVisible);
      clearTimeout(timer);
      if (held) URL.revokeObjectURL(held);
      // Leaving the tab lets the device go: the core shuts down a simulator it started for this tab.
      if (asked) void core.request('devices.close', { taskId }).catch(() => {});
    });
  });

  // Reload, or another address: the address is opened again on the device that is shown.
  createEffect(
    on(
      () => [props.reloads, props.address] as const,
      () => {
        if (!shown()) return;
        // The app is gone: there is nothing to show on the device, so it is let go.
        if (!props.address) void leave();
        else if (current) void open(current);
      },
      { defer: true },
    ),
  );

  return (
    <Switch>
      <Match when={!found()}>
        <p class="bottom-note">Looking for {words.many}</p>
      </Match>
      <Match when={found()!.devices.length === 0}>
        <div class="center-note">
          <h2>No {words.one} to show</h2>
          <p>{found()!.needs}</p>
          <div class="actions">
            <button type="button" class="btn small" onClick={() => void look()}>
              Look again
            </button>
          </div>
        </div>
      </Match>
      <Match when={!props.address}>{props.children}</Match>
      <Match when={shown()}>
        {(device) => (
          <section class="device" aria-label={`${device().name}, ${words.one}`}>
            <div class="device-head">
              <h3 class="grow">{device().name}</h3>
              <Show when={kind === 'ios'}>
                <button type="button" class="btn small" onClick={front}>
                  Bring the Simulator forward
                </button>
              </Show>
              <button type="button" class="btn small ghost" onClick={() => void leave().then(look)}>
                Another {words.one}
              </button>
            </div>
            <p class="fine device-note">
              This is a picture of the {words.one}'s screen, taken again every second. Tap and type in {words.window}.
              <Show when={kind === 'ios'}>
                {' '}
                {device().startedHere
                  ? 'Agent Editor started this simulator and shuts it down when you leave this tab.'
                  : 'It was running already, so it stays on when you leave this tab.'}
              </Show>
            </p>
            <Show when={problem()}>
              <p class="notice danger" role="alert">
                {problem()}
              </p>
            </Show>
            <div class="device-screen">
              <Show
                when={picture()}
                fallback={
                  <p class="bottom-note">
                    {problem() ? 'No picture of its screen right now' : 'Waiting for the first picture of its screen'}
                  </p>
                }
              >
                <img src={picture()!} alt={`The screen of ${device().name}`} />
              </Show>
            </div>
            <div class="statusline device-status">
              <span>
                {device().address} on {device().name}
              </span>
              <Show when={opening()}>
                <span>Opening the address again.</span>
              </Show>
              <Show when={takenAt()}>{(at) => <span>Picture taken at {toTheSecond(at())}.</span>}</Show>
            </div>
          </section>
        )}
      </Match>
      <Match when={opening()}>
        {(device) => (
          <div class="center-note" role="status">
            <h2>{device().state === 'off' ? `Starting ${device().name}` : `Opening the app on ${device().name}`}</h2>
            <p>
              {device().state === 'off'
                ? `A ${words.one} that is off takes a while to start, and one that never ran takes longer. Then it opens ${props.address}.`
                : `It opens ${props.address}.`}
            </p>
          </div>
        )}
      </Match>
      <Match when={true}>
        <div class="center-note">
          <h2>Pick {kind === 'ios' ? 'a simulator' : 'an emulator'}</h2>
          <p>
            It opens {props.address}, and its screen is shown here as a picture that is taken again every second.
            Tapping and typing happen in {words.window}.
          </p>
          <Show when={problem()}>
            <p class="notice danger" role="alert">
              {problem()}
            </p>
          </Show>
          <ul class="device-list" aria-label={words.many}>
            <For each={found()!.devices}>
              {(device) => (
                <li>
                  <span class="device-name">{device.name}</span>
                  <span class="fine grow">{deviceLine(device)}</span>
                  <button
                    type="button"
                    class="btn small"
                    disabled={!deviceButton(device).can}
                    aria-label={`${deviceButton(device).label} ${device.name}`}
                    onClick={() => void open(device)}
                  >
                    {deviceButton(device).label}
                  </button>
                </li>
              )}
            </For>
          </ul>
          <div class="actions">
            <button type="button" class="btn small ghost" onClick={() => void look()}>
              Look again
            </button>
          </div>
        </div>
      </Match>
    </Switch>
  );
}
