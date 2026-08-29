import type { Envelope, EncryptedEnvelope } from "../types/relay-protocol";
import { isInsecureDevEndpoint } from "../pairing/qr-parse";
import { diagnostics } from "../util/diagnostics";

/** Hard cap for a single inbound WS text frame (4 MiB). Frames above this
 * are dropped defensively to avoid OOM/crash on memory-constrained phones. */
const MAX_FRAME_BYTES = 4 * 1024 * 1024;

// Abstract duplex text-frame transport. The real app uses a TLS WebSocket
// (`WsTransport`); tests use an in-memory `ChannelTransport` (no networking).
// Carries raw JSON text so the connection layer can choose plain `Envelope`
// or `EncryptedEnvelope` framing.
export interface RelayTransport {
  sendText(frame: string): Promise<void>;
  recvText(): Promise<string | null>;
  close(): Promise<void>;
}

/** Reject non-TLS endpoints unless the debug flag is set or the endpoint is
 * a dev-shaped plain-`ws://` target (bare IP / localhost / *.local) that has
 * no TLS identity to protect anyway. Mirrors `parsePairingQr`. */
export function assertTlsEndpoint(url: string, allowInsecureDebug: boolean): void {
  if (url.startsWith("wss://")) return;
  if (url.startsWith("ws://") && (allowInsecureDebug || isInsecureDevEndpoint(url))) return;
  throw new Error(`refusing non-TLS relay endpoint: ${url}`);
}

/**
 * A `WebSocket`-backed transport. Uses the platform global `WebSocket`
 * (Node 22+ and React Native both provide it). Not unit-tested (needs a real
 * WS server); the in-memory `ChannelTransport` covers connection logic.
 */
export class WsTransport implements RelayTransport {
  private ws: WebSocket;
  private incoming: string[] = [];
  private waiters: Array<(v: string | null) => void> = [];
  private closed = false;

  constructor(url: string) {
    this.ws = new WebSocket(url);
    this.ws.binaryType = "arraybuffer";
    this.ws.onopen = () => {
      diagnostics.log("conn", `ws open ${url}`);
    };
    this.ws.onmessage = (ev: MessageEvent) => {
      const data =
        typeof ev.data === "string"
          ? ev.data
          : new TextDecoder().decode(ev.data as ArrayBuffer);
      // Guard against multi-megabyte frames that can OOM React Native's WS
      // stack before the receive loop can process them. The server should not
      // send these after the snapshot-size fix; dropping defensively here keeps
      // the connection (and app) alive instead of crashing on a giant frame.
      if (data.length > MAX_FRAME_BYTES) {
        diagnostics.log(
          "conn",
          `ws dropped oversized frame (${data.length} bytes > ${MAX_FRAME_BYTES})`,
        );
        return;
      }
      const waiter = this.waiters.shift();
      if (waiter) waiter(data);
      else this.incoming.push(data);
    };
    this.ws.onclose = (ev: CloseEvent) => {
      // Surface close codes on-device: an abrupt code (1006/abnormal, or a
      // native stack abort) vs a clean 1000 distinguishes "phone-side crash
      // killed the socket" from "relay closed us", which is exactly the
      // signal needed to diagnose reconnect storms.
      diagnostics.log(
        "conn",
        `ws closed code=${ev?.code ?? "?"} wasClean=${ev?.wasClean ?? "?"}`,
      );
      this.closed = true;
      while (this.waiters.length) this.waiters.shift()!(null);
    };
    this.ws.onerror = () => {
      diagnostics.log("conn", "ws error");
      this.closed = true;
      while (this.waiters.length) this.waiters.shift()!(null);
    };
  }

  get ready(): Promise<void> {
    return new Promise((resolve, reject) => {
      if (this.ws.readyState === WebSocket.OPEN) return resolve();
      this.ws.onopen = () => resolve();
      this.ws.onerror = () => reject(new Error("ws open failed"));
    });
  }

  async sendText(frame: string): Promise<void> {
    if (this.closed) throw new Error("transport closed");
    this.ws.send(frame);
  }

  async recvText(): Promise<string | null> {
    const queued = this.incoming.shift();
    if (queued !== undefined) return queued;
    if (this.closed) return null;
    return new Promise<string | null>((resolve) => this.waiters.push(resolve));
  }

  async close(): Promise<void> {
    this.closed = true;
    try {
      this.ws.close();
    } catch {
      // ignore
    }
  }
}

/** Heuristic: does this text frame look like an EncryptedEnvelope? Accepts
 * both ciphertext encodings: the legacy number array and the compact
 * base64url `ciphertext_b64` emitted by newer PCs. */
export function looksLikeEncrypted(frame: string): boolean {
  try {
    const obj = JSON.parse(frame) as Partial<Envelope & EncryptedEnvelope>;
    return (
      typeof (obj as EncryptedEnvelope).to_device_id === "string" &&
      (Array.isArray((obj as EncryptedEnvelope).ciphertext) ||
        typeof (obj as EncryptedEnvelope).ciphertext_b64 === "string")
    );
  } catch {
    return false;
  }
}
