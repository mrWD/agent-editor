import { createEffect, createSignal, For, Match, on, onCleanup, onMount, Show, Switch, type JSX } from 'solid-js';
import { core } from '../core/client';
import {
  deviceButton,
  deviceKeys,
  deviceLine,
  deviceSetup,
  deviceWords,
  readDevices,
  touchOn,
  type Device,
  type DeviceKind,
  type Devices,
  type DeviceTouch,
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

/** How soon after a touch the screen is asked for again: the device needs a moment to draw what the touch did. */
const AFTER_TOUCH_MS = 250;

/** The device a tab shows, as the core said when it opened the address on it. */
type Shown = {
  id: string;
  name: string;
  address: string;
  startedHere: boolean;
  /** Whether a touch on the picture can be passed on to the device. */
  touch?: boolean;
  /** What this computer needs for that, when it cannot. */
  touchNeeds?: string | null;
};

/**
 * A device tab of the Preview: the iOS simulator or the Android emulator.
 *
 * A device cannot be put into a page. What is shown is a picture of its screen,
 * asked of the core again about once a second for as long as the tab is on
 * screen. A press on the picture is a tap on the device and a drag is a swipe;
 * text is typed into a field under it, next to the device's own buttons. A
 * simulator takes that only on a Mac with `idb`; without it the tab says so and
 * tapping stays in the Simulator window. Leaving the tab tells the core to let
 * the device go.
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
  /** What is being typed for the device. */
  const [typed, setTyped] = createSignal('');
  /** Whether the device was last made dark from here. Nobody knows how it looked before. */
  const [dark, setDark] = createSignal<boolean | null>(null);
  /** Where on the page a press on the picture began. */
  let pressed: { x: number; y: number } | null = null;
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
      // The tab was left, or "Another device" was pressed while this was on its way: the core lets the
      // device go right after, so it is not put back on show.
      if (gone || current !== device) return;
      if (shown()?.id !== opened.id) forgetPicture();
      setShown(opened);
      void watch();
    } catch (error) {
      if (gone || current !== device) return;
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
    // What was typed and how the device was made to look were this device's: the next one starts clean.
    setTyped('');
    setDark(null);
    if (asked) await core.request('devices.close', { taskId }).catch(() => {});
  };

  const front = () => {
    setProblem(null);
    core.request('devices.front', { taskId }).catch((error) => setProblem(message(error)));
  };

  /** The command that was copied last, so its button can say so for a moment. */
  const [copied, setCopied] = createSignal<string | null>(null);
  const copy = async (command: string) => {
    try {
      await navigator.clipboard.writeText(command);
      setCopied(command);
      setTimeout(() => copied() === command && setCopied(null), 2000);
    } catch {
      // No clipboard here: the command stands on the page to be selected by hand.
    }
  };

  /** Passes a touch on to the device, and looks at its screen again a moment later. */
  const touch = async (what: DeviceTouch) => {
    setProblem(null);
    try {
      await core.request('devices.input', { taskId, ...what });
      clearTimeout(timer);
      timer = setTimeout(() => void watch(), AFTER_TOUCH_MS);
      return true;
    } catch (error) {
      if (!gone) setProblem(message(error));
      return false;
    }
  };

  /** The picture without its frame, as it stands on the page. */
  const screenBox = (image: HTMLImageElement) => {
    const box = image.getBoundingClientRect();
    return {
      left: box.left + image.clientLeft,
      top: box.top + image.clientTop,
      width: image.clientWidth,
      height: image.clientHeight,
    };
  };

  const onPress = (event: PointerEvent & { currentTarget: HTMLImageElement }) => {
    if (event.button !== 0) return;
    pressed = { x: event.clientX, y: event.clientY };
    // The drag may end outside the picture: its end is still told to the picture.
    event.currentTarget.setPointerCapture?.(event.pointerId);
    event.preventDefault();
  };

  const onRelease = (event: PointerEvent & { currentTarget: HTMLImageElement }) => {
    const began = pressed;
    pressed = null;
    if (!began) return;
    const what = touchOn(screenBox(event.currentTarget), began, { x: event.clientX, y: event.clientY });
    if (what) void touch(what);
  };

  const type = async (event: SubmitEvent) => {
    event.preventDefault();
    const text = typed();
    if (text && (await touch({ what: 'text', text }))) setTyped('');
  };

  const setScheme = (toDark: boolean) => {
    setProblem(null);
    core
      .request('devices.scheme', { taskId, scheme: toDark ? 'dark' : 'light' })
      .then(() => {
        setDark(toDark);
        clearTimeout(timer);
        timer = setTimeout(() => void watch(), AFTER_TOUCH_MS);
      })
      .catch((error) => setProblem(message(error)));
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
      (now, before) => {
        // The address is worked out anew each time the state of what runs is read, and mostly comes out
        // the same: only a press on Reload or another address opens it on the device again.
        if (before && now[0] === before[0] && now[1] === before[1]) return;
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
          <Show when={deviceSetup(kind, found()!.needs).length > 0}>
            <ol class="device-steps" aria-label={`How to get ${kind === 'ios' ? 'a simulator' : 'an emulator'}`}>
              <For each={deviceSetup(kind, found()!.needs)}>
                {(step) => (
                  <li>
                    <span>{step.text}</span>
                    <Show when={step.command}>
                      {(command) => (
                        <span class="device-command">
                          <code>{command()}</code>
                          <button
                            type="button"
                            class="btn tiny ghost"
                            aria-label={`Copy the command: ${command()}`}
                            onClick={() => void copy(command())}
                          >
                            {copied() === command() ? 'Copied' : 'Copy'}
                          </button>
                        </span>
                      )}
                    </Show>
                  </li>
                )}
              </For>
            </ol>
          </Show>
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
              This is a picture of the {words.one}'s screen, taken again every second.{' '}
              {device().touch
                ? 'Press on it to tap, drag to swipe, and type in the field under it.'
                : (device().touchNeeds ?? `Tap and type in ${words.window}.`)}{' '}
              {device().startedHere
                ? `Agent Editor started this ${words.one} and shuts it down when you leave this tab.`
                : 'It was running already, so it stays on when you leave this tab.'}
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
                <img
                  src={picture()!}
                  alt={`The screen of ${device().name}`}
                  classList={{ touchable: Boolean(device().touch) }}
                  draggable={false}
                  onPointerDown={(event) => device().touch && onPress(event)}
                  onPointerUp={(event) => device().touch && onRelease(event)}
                  onPointerCancel={() => (pressed = null)}
                />
              </Show>
            </div>
            <div class="device-keys" role="group" aria-label={`The buttons of ${device().name}`}>
              <Show when={device().touch}>
                <For each={deviceKeys(kind)}>
                  {(one) => (
                    <button type="button" class="btn small" onClick={() => void touch({ what: 'key', key: one.key })}>
                      {one.label}
                    </button>
                  )}
                </For>
                <form class="device-type" onSubmit={(event) => void type(event)}>
                  <input
                    type="text"
                    aria-label={`Type on ${device().name}`}
                    placeholder="Type on the device"
                    autocomplete="off"
                    spellcheck={false}
                    value={typed()}
                    onInput={(event) => setTyped(event.currentTarget.value)}
                  />
                  <button type="submit" class="btn small" disabled={!typed()}>
                    Type
                  </button>
                </form>
                <button type="button" class="btn small ghost" onClick={() => void touch({ what: 'key', key: 'enter' })}>
                  Enter
                </button>
                <button
                  type="button"
                  class="btn small ghost"
                  aria-label="Delete the letter before the cursor"
                  onClick={() => void touch({ what: 'key', key: 'delete' })}
                >
                  Delete
                </button>
              </Show>
              <span class="grow" />
              <div class="switcher" role="group" aria-label={`How ${device().name} looks`}>
                <button type="button" aria-pressed={dark() === false} onClick={() => setScheme(false)}>
                  Light
                </button>
                <button type="button" aria-pressed={dark() === true} onClick={() => setScheme(true)}>
                  Dark
                </button>
              </div>
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
            {kind === 'android'
              ? ' A press on the picture is a tap on the emulator. One that is off is started here and shut down again when you leave this tab.'
              : ' One that is off is started here and shut down again when you leave this tab.'}
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
