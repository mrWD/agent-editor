import { createSignal } from 'solid-js';

/** Something the core sent without being asked. */
export type CoreEvent = { event: string; data: any };

export type LinkStatus = 'connecting' | 'open' | 'lost';

type Waiting = { resolve: (value: any) => void; reject: (reason: Error) => void };

/**
 * The one connection to the core.
 *
 * Commands are numbered, and each answer carries the number of its command.
 * When the connection drops it is retried, and listeners get a `link.open`
 * event so they can reload what they show.
 */
class CoreClient {
  private socket: WebSocket | null = null;
  private nextId = 1;
  private waiting = new Map<number, Waiting>();
  private listeners = new Set<(event: CoreEvent) => void>();
  private binaryListeners = new Set<(kind: number, channel: number, bytes: Uint8Array) => void>();
  private opened: Promise<void> = Promise.resolve();
  private attempts = 0;
  private started = false;

  readonly status;
  private setStatus;

  constructor() {
    const [status, setStatus] = createSignal<LinkStatus>('connecting');
    this.status = status;
    this.setStatus = setStatus;
  }

  /** Opens the connection. Safe to call more than once. */
  start() {
    if (this.started) return;
    this.started = true;
    this.connect();
  }

  private connect() {
    const scheme = location.protocol === 'https:' ? 'wss' : 'ws';
    const socket = new WebSocket(`${scheme}://${location.host}/ws`);
    socket.binaryType = 'arraybuffer';
    this.socket = socket;
    this.opened = new Promise((resolve) => {
      socket.addEventListener('open', () => {
        this.attempts = 0;
        this.setStatus('open');
        resolve();
        this.publish({ event: 'link.open', data: {} });
      });
    });

    socket.addEventListener('message', (message) => {
      // Raw bytes, such as terminal output: a kind, a channel number, then the bytes.
      if (message.data instanceof ArrayBuffer) {
        if (message.data.byteLength < 5) return;
        const view = new DataView(message.data);
        const bytes = new Uint8Array(message.data, 5);
        for (const listener of this.binaryListeners) listener(view.getUint8(0), view.getUint32(1), bytes);
        return;
      }
      let parsed: any;
      try {
        parsed = JSON.parse(String(message.data));
      } catch {
        return;
      }
      if (typeof parsed.event === 'string') {
        this.publish(parsed as CoreEvent);
        return;
      }
      const waiter = this.waiting.get(parsed.id);
      if (!waiter) return;
      this.waiting.delete(parsed.id);
      if (parsed.error) waiter.reject(new Error(parsed.error.message ?? 'The core refused the command'));
      else waiter.resolve(parsed.result);
    });

    socket.addEventListener('close', () => {
      if (this.socket !== socket) return;
      this.setStatus('lost');
      for (const waiter of this.waiting.values()) waiter.reject(new Error('The connection to the core was lost'));
      this.waiting.clear();
      const delay = Math.min(5000, 300 * 2 ** this.attempts++);
      setTimeout(() => this.connect(), delay);
    });
  }

  private publish(event: CoreEvent) {
    for (const listener of this.listeners) listener(event);
  }

  /** Sends a command and waits for its answer. */
  async request<T = unknown>(method: string, params: Record<string, unknown> = {}): Promise<T> {
    await this.opened;
    const socket = this.socket;
    if (!socket || socket.readyState !== WebSocket.OPEN) {
      throw new Error('The core is not connected');
    }
    const id = this.nextId++;
    return new Promise<T>((resolve, reject) => {
      this.waiting.set(id, { resolve, reject });
      socket.send(JSON.stringify({ id, method, params }));
    });
  }

  /** Sends raw bytes on a numbered channel, such as keystrokes to a terminal. */
  sendBytes(kind: number, channel: number, bytes: Uint8Array) {
    const socket = this.socket;
    if (!socket || socket.readyState !== WebSocket.OPEN) return;
    const message = new Uint8Array(5 + bytes.length);
    message[0] = kind;
    new DataView(message.buffer).setUint32(1, channel);
    message.set(bytes, 5);
    socket.send(message);
  }

  /** Calls `listener` for every piece of raw bytes. Returns a function that stops it. */
  onBytes(listener: (kind: number, channel: number, bytes: Uint8Array) => void): () => void {
    this.binaryListeners.add(listener);
    return () => this.binaryListeners.delete(listener);
  }

  /** Calls `listener` for every event. Returns a function that stops it. */
  onEvent(listener: (event: CoreEvent) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }
}

export const core = new CoreClient();
