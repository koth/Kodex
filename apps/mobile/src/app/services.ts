import type { RelayTransport } from "../relay/transport";
import { RelayConnection } from "../relay/connection";
import { ControlClient } from "../session/control-client";
import { SessionStore } from "../session/store";
import {
  PermissionApprovalStore,
  type PendingApproval,
} from "../session/permission";
import { ConnectionStateMachine, type ConnectionState } from "../relay/state-machine";
import { runReceiveLoop, type EventSink } from "../relay/driver";
import type { DeviceIdentity, SecretStore } from "../crypto/identity";
import { loadOrCreateIdentity, deviceId } from "../crypto/identity";
import type { SessionKey } from "../crypto/session-key";
import type { PairingQrPayload, Envelope, EventFrame, SubscriptionStatus } from "../types/relay-protocol";
import { fromMessage } from "../relay/framing";
import { parsePairingQr } from "../pairing/qr-parse";
import {
  runPairingHandshake,
  runPairingResume,
  buildDeviceAuthArgs,
} from "../pairing/pairing-flow";
import type { UiSnapshot, PermissionInputResponse } from "../types";
import {
  loadBoundDevice,
  persistBoundDevice,
  clearBoundDevice,
  canReconnectWithoutRescan,
} from "../account/binding";
import type { BoundDevice } from "../account/binding";
import { diagnostics } from "../util/diagnostics";
import {
  loadSession,
  persistSession,
  clearSession,
} from "../account/session";
import {
  subscriptionStateFromStatus,
  demoteOnExpiry,
  NO_SUBSCRIPTION,
  type SubscriptionState,
} from "../account/subscription";

// Framework-agnostic controller wiring the relay connection, control client,
// session store, permission store, and connection state machine. The React
// provider in `AppServicesContext` constructs one with a real `WsTransport`
// factory + `SecureSecretStore`; the integration harness builds one with an
// in-memory `ChannelTransport`. Fail-open: connection errors are surfaced as
// state transitions, never thrown to the UI.
type ReconnectTransportFactory = (endpoint: string) => Promise<RelayTransport>;

export class AppController {
  readonly sessionStore = new SessionStore();
  readonly connState = new ConnectionStateMachine();
  readonly permissions = new PermissionApprovalStore(null);
  private readonly secretStore: SecretStore;
  private identity: DeviceIdentity | null = null;
  private conn: RelayConnection | null = null;
  private control: ControlClient | null = null;
  private loopPromise: Promise<void> | null = null;
  private heartbeatTimer: ReturnType<typeof setInterval> | null = null;
  private connectingPromise: Promise<boolean> | null = null;
  private reconnectAttempt = 0;
  private connectionGeneration = 0;
  private lastSessionKey: SessionKey | null = null;
  private lastPeerDeviceId: string | null = null;
  private stopLoop = false;
  private subscription: SubscriptionState = { ...NO_SUBSCRIPTION };
  private onSubscriptionChange: ((state: SubscriptionState) => void) | null = null;
  private readonly reconnectTransportFactory: ReconnectTransportFactory | null;

  constructor(
    secretStore: SecretStore,
    reconnectTransportFactory?: ReconnectTransportFactory,
  ) {
    this.secretStore = secretStore;
    this.reconnectTransportFactory = reconnectTransportFactory ?? null;
    // Surface/dismiss pending permissions from the snapshot: the phone derives
    // the permission_request_id (== tool call_id) from the tool, since the
    // EventFrame::PermissionRequest carries only the PermissionInputRequest.
    this.sessionStore.setPermissionHandler(() => this.rescanPendingPermissions());
    this.sessionStore.subscribe(() => this.rescanPendingPermissions());
  }

  get connectionState(): ConnectionState {
    return this.connState.state;
  }

  get snapshot(): UiSnapshot | null {
    return this.sessionStore.state;
  }

  get subscriptionState(): SubscriptionState {
    return this.subscription;
  }

  get deviceIdValue(): string | null {
    return this.identity ? deviceId(this.identity) : null;
  }

  get pendingApprovals(): PendingApproval[] {
    return this.permissions.snapshot();
  }

  /** Load (or create) the persistent device identity. Idempotent. */
  async ensureIdentity(): Promise<DeviceIdentity> {
    if (this.identity) return this.identity;
    this.identity = await loadOrCreateIdentity(this.secretStore);
    return this.identity;
  }

  setSubscriptionListener(fn: (state: SubscriptionState) => void): void {
    this.onSubscriptionChange = fn;
  }

  /**
   * Pair with a PC from a scanned QR payload (JSON). Dials `transport` (already
   * connected for the real WebSocket path), runs DeviceAuth + the E2E
   * handshake, installs the session key, starts the receive loop, and resyncs
   * state via GetState. Throws on protocol/transport failure.
   */
  async pairFromTransport(
    transport: RelayTransport,
    qrJson: string,
    allowInsecureWs = false,
  ): Promise<void> {
    const qr = parsePairingQr(qrJson, allowInsecureWs) as PairingQrPayload;
    const identity = await this.ensureIdentity();
    this.connState.transition("connecting");
    this.conn = new RelayConnection(transport);

    this.connState.transition("authenticating");
    const auth = buildDeviceAuthArgs(identity);
    await this.conn.authenticate(auth.deviceId, auth.devicePubkey, auth.signature, auth.timestampMs);

    this.connState.transition("paired/e2e");
    const result = await runPairingHandshake(this.conn, identity, qr);
    const bound: BoundDevice = {
      device_id: deviceId(identity),
      // The free/bound account path retains a resumable pairing token after
      // the first scan. `auth_token` is empty until the account bind flow can
      // fill it; resume only needs the pairing token + peer static key.
      auth_token: "",
      pairing_token: result.pairingToken,
      peer_device_id: result.pcDeviceId,
      peer_static_pubkey_b64: result.pcStaticPubkeyB64,
      relay_endpoint: qr.relay_endpoint,
    };
    await persistBoundDevice(this.secretStore, bound);
    // Diagnostic: log session key prefix + peer id so it can be matched
    // against the PC's derived key. (Hermes console -> logcat ReactNativeJS.)
    const keyHex = Array.from(result.sessionKey.bytes.slice(0, 8))
      .map((b) => b.toString(16).padStart(2, "0"))
      .join("");
    diagnostics.log(
      "pairing",
      `sessionKey prefix=${keyHex} pcDeviceId=${result.pcDeviceId} phoneDeviceId=${result.phoneDeviceId}`,
    );
    this.conn.installSessionKey(result.sessionKey, result.pcDeviceId);
    this.lastSessionKey = result.sessionKey;
    this.lastPeerDeviceId = result.pcDeviceId;
    await persistSession(this.secretStore, {
      key: result.sessionKey,
      peer_device_id: result.pcDeviceId,
    });

    this.control = new ControlClient(this.conn);
    this.permissions.setControlClient(this.control);
    this.connState.transition("connected");

    const generation = ++this.connectionGeneration;
    this.stopLoop = false;
    this.loopPromise = this.runLoop(generation).catch(() => {});
    this.startHeartbeat();
  }

  private async runLoop(generation: number): Promise<void> {
    if (!this.conn || !this.control) return;
    const onEvent: EventSink = (frame: EventFrame) =>
      this.sessionStore.applyEventFrame(frame);
    const onOther = (env: Envelope) => this.handleOther(env);
    await runReceiveLoop(
      this.conn,
      this.control,
      onEvent,
      onOther,
      () => this.stopLoop,
    );
    if (this.stopLoop || generation !== this.connectionGeneration) return;
    void this.handleConnectionLoss();
  }

  /** A receive loop ended. Reconnect when a persisted pairing exists, or
   * surface disconnected otherwise. */
  private async handleConnectionLoss(): Promise<void> {
    this.stopHeartbeat();
    if (this.stopLoop) {
      this.connState.transition("disconnected");
      return;
    }
    const resumed = await this.tryAutoResume();
    if (!resumed && !this.stopLoop) {
      this.connState.transition("disconnected");
    }
  }

  private sleep(ms: number): Promise<void> {
    return new Promise((resolve) => setTimeout(resolve, ms));
  }

  private startHeartbeat(): void {
    this.stopHeartbeat();
    this.heartbeatTimer = setInterval(() => {
      if (!this.conn || this.stopLoop) return;
      // Plaintext heartbeat: the relay must see the frame to keep the
      // connection alive. An encrypted envelope carries `to_device_id` and
      // is routed to the peer instead of being consumed by the relay, so
      // the relay's heartbeat_timeout would reap this connection.
      this.conn
        .sendPlaintext(fromMessage(null, { type: "heartbeat", payload: null }))
        .catch(() => {});
    }, 20_000);
  }

  private stopHeartbeat(): void {
    if (this.heartbeatTimer !== null) {
      clearInterval(this.heartbeatTimer);
      this.heartbeatTimer = null;
    }
  }

  /** Route non-control/non-event envelopes (subscription/bind messages). */
  private handleOther(env: Envelope): void {
    if (env.type === "subscription_status") {
      const status = env.payload as SubscriptionStatus;
      const { state, mustRescan } = demoteOnExpiry(this.subscription, status);
      this.subscription = state;
      this.onSubscriptionChange?.(state);
      // On expiry we demote to re-scan semantics but keep the session alive.
      if (mustRescan) this.sessionStore.clear();
    }
  }

  /** Best-effort bound reconnect: re-scan is required unless bound+active. */
  async canReconnectWithoutRescan(): Promise<boolean> {
    const bound = await loadBoundDevice(this.secretStore);
    return canReconnectWithoutRescan(this.subscription.active, bound);
  }

  async loadBoundIfAny(): Promise<boolean> {
    const bound = await loadBoundDevice(this.secretStore);
    return bound !== null && this.subscription.active;
  }

  /**
   * Reconnect to the previously bound PC without scanning a fresh QR. The
   * persisted `BoundDevice` supplies the pairing token and PC static public
   * key; this runs DeviceAuth + a fresh E2E resume handshake, installs the
   * derived key, and restarts the receive loop.
   */
  private async establishBoundConnection(
    transport: RelayTransport,
    bound: BoundDevice,
  ): Promise<void> {
    const identity = await this.ensureIdentity();
    this.connState.transition("connecting");
    this.conn = new RelayConnection(transport);

    this.connState.transition("authenticating");
    const auth = buildDeviceAuthArgs(identity);
    await this.conn.authenticate(auth.deviceId, auth.devicePubkey, auth.signature, auth.timestampMs);

    this.connState.transition("paired/e2e");
    const result = await runPairingResume(this.conn, bound);
    this.conn.installSessionKey(result.sessionKey, result.pcDeviceId);
    this.lastSessionKey = result.sessionKey;
    this.lastPeerDeviceId = result.pcDeviceId;
    await persistSession(this.secretStore, {
      key: result.sessionKey,
      peer_device_id: result.pcDeviceId,
    });

    this.control = new ControlClient(this.conn);
    this.permissions.setControlClient(this.control);
  }

  /**
   * Reconnect with the same E2E key already established in this process.
   * This keeps a transient relay/network drop seamless without asking the PC
   * to re-run a pairing handshake. Only used when both sides still share the
   * in-memory key from the original pairing.
   */
  private async establishCachedConnection(
    transport: RelayTransport,
    key: SessionKey,
    peerDeviceId: string,
  ): Promise<void> {
    const identity = await this.ensureIdentity();
    this.connState.transition("connecting");
    this.conn = new RelayConnection(transport);

    this.connState.transition("authenticating");
    const auth = buildDeviceAuthArgs(identity);
    await this.conn.authenticate(auth.deviceId, auth.devicePubkey, auth.signature, auth.timestampMs);

    this.conn.installSessionKey(key, peerDeviceId);
    this.control = new ControlClient(this.conn);
    this.permissions.setControlClient(this.control);
  }

  /**
   * Reconnect using a persisted pairing token + peer static key, dialing a
   * fresh transport when only the endpoint is known. Used both for app
   * startup and for live-drop recovery.
   */
  async resumeFromBoundTransport(transport?: RelayTransport): Promise<void> {
    const bound = await loadBoundDevice(this.secretStore);
    if (!bound) {
      throw new Error("no persisted pairing to resume");
    }
    const endpoint = bound.relay_endpoint;
    if (!transport && !endpoint) {
      throw new Error("persisted pairing is missing a relay endpoint");
    }

    if (transport) {
      if (this.lastSessionKey && this.lastPeerDeviceId) {
        await this.establishCachedConnection(
          transport,
          this.lastSessionKey,
          this.lastPeerDeviceId,
        );
      } else {
        await this.establishBoundConnection(transport, bound);
      }
    } else {
      if (!this.reconnectTransportFactory) {
        throw new Error("no transport factory configured for resume");
      }
      const ws = await this.reconnectTransportFactory(endpoint!);
      if (this.lastSessionKey && this.lastPeerDeviceId) {
        await this.establishCachedConnection(
          ws,
          this.lastSessionKey,
          this.lastPeerDeviceId,
        );
      } else {
        await this.establishBoundConnection(ws, bound);
      }
    }

    const generation = ++this.connectionGeneration;
    this.stopLoop = false;
    this.loopPromise = this.runLoop(generation).catch(() => {});
    this.startHeartbeat();
  }

  /** Attempt the persisted pairing resume without input (startup recovery). */
  async tryAutoResume(): Promise<boolean> {
    if (this.connectingPromise) {
      return this.connectingPromise;
    }
    this.connectingPromise = this.doAutoResume().finally(() => {
      this.connectingPromise = null;
    });
    return this.connectingPromise;
  }

  private async doAutoResume(): Promise<boolean> {
    while (!this.stopLoop) {
      try {
        diagnostics.log("services", `resume attempt ${this.reconnectAttempt + 1}`);
        await this.resumeFromBoundTransport();
        if (this.control) {
          await this.control.getState();
        }
        this.connState.transition("connected");
        this.reconnectAttempt = 0;
        diagnostics.log("services", "resume succeeded");
        return true;
      } catch (e) {
        diagnostics.log("services", `resume failed: ${e instanceof Error ? e.message : String(e)}`);
        if (this.lastSessionKey && this.lastPeerDeviceId) {
          this.lastSessionKey = null;
          this.lastPeerDeviceId = null;
          await clearSession(this.secretStore);
        }
        this.reconnectAttempt += 1;
        this.connState.transition("connecting");
        if (this.reconnectAttempt >= 8) {
          this.connState.transition("disconnected");
          return false;
        }
        const delay = Math.min(500 * 2 ** (this.reconnectAttempt - 1), 8_000);
        await this.sleep(delay);
      }
    }
    return false;
  }

  /** App startup: mark the connection as bootstrapping, then attempt a
   * persisted resume. Returns true if a connection was established. */
  async boot(): Promise<boolean> {
    await this.ensureIdentity();
    this.connState.transition("disconnected");
    return false;
  }

  async unbindAndClear(): Promise<void> {
    await clearBoundDevice(this.secretStore);
    await clearSession(this.secretStore);
    this.lastSessionKey = null;
    this.lastPeerDeviceId = null;
  }

  setSubscriptionFromStatus(status: SubscriptionStatus): void {
    this.subscription = subscriptionStateFromStatus(status);
    this.onSubscriptionChange?.(this.subscription);
  }

  /** Surface pending permissions from the snapshot (the snapshot is the
   * source of the tool call_id, which is the permission_request_id on the
   * wire). */
  private rescanPendingPermissions(): void {
    const snap = this.sessionStore.state;
    const pending = this.permissions.snapshot();
    const pendingIds = new Set(pending.map((p) => p.permissionRequestId));
    if (snap) {
      for (const tool of snap.tools) {
        if (
          tool.permission_input &&
          !tool.permission_decision &&
          !pendingIds.has(tool.call_id)
        ) {
          this.permissions.surface(
            tool.call_id,
            { name: tool.name, kind: tool.kind, id: tool.id, call_id: tool.call_id },
            tool.permission_input,
          );
        }
      }
    }
    // Dismiss approvals whose tool is no longer pending (resolved/denied).
    const stillPending = new Set(
      (snap?.tools ?? [])
        .filter((t) => t.permission_input && !t.permission_decision)
        .map((t) => t.call_id),
    );
    for (const p of pending) {
      if (!stillPending.has(p.permissionRequestId)) {
        this.permissions.dismiss(p.permissionRequestId);
      }
    }
  }

  // --- Session control ops (proxy to the control client) ---

  async listSessions() {
    return this.controlClient().listSessions();
  }

  async createSession(opts?: { workspaceRoot?: string | null; agent?: string | null }) {
    const res = await this.controlClient().createSession({
      workspace_root: opts?.workspaceRoot ?? null,
      agent: (opts?.agent ?? null) as never,
    });
    this.sessionStore.beginSession(res.session_id);
    return res.session_id;
  }

  async switchSession(sessionId: string, workspaceRoot?: string | null) {
    this.sessionStore.beginSession(sessionId);
    return this.controlClient().switchSession(sessionId, workspaceRoot ?? null);
  }

  /** Fetch and install the active session's full snapshot. Call after
   * switching, not on every pairing/list refresh. */
  async getState(expectedSessionId?: string): Promise<void> {
    const response = await this.controlClient().getState();
    if (expectedSessionId && response.snapshot.session.id !== expectedSessionId) {
      throw new Error("switched session state mismatch");
    }
    this.sessionStore.setSnapshot(response.snapshot);
  }

  async sendPrompt(text: string) {
    return this.controlClient().sendPrompt([
      { type: "text", text } as never,
    ]);
  }

  async cancel() {
    return this.controlClient().cancel();
  }

  async stopTool(toolCallId: string) {
    return this.controlClient().stopTool(toolCallId);
  }

  /** Approve a pending permission by its (call_id) id and chosen option id. */
 async approvePermission(
   permissionRequestId: string,
   optionId: string | null,
   guidance?: string | null,
 ) {
   await this.permissions.approve(permissionRequestId, optionId, guidance ?? null, null);
 }

  async approvePermissionWithInput(
    permissionRequestId: string,
    optionId: string | null,
    guidance: string | null,
    inputResponse: PermissionInputResponse,
  ) {
    await this.permissions.approve(permissionRequestId, optionId, guidance, inputResponse);
  }

  async denyPermission(permissionRequestId: string, guidance?: string | null) {
    await this.permissions.deny(permissionRequestId, guidance ?? null);
  }

  /** Install a session key directly (used by bound reconnect / tests). */
  installSessionKey(key: SessionKey, peerDeviceId: string): void {
    if (!this.conn) throw new Error("no connection to install session key on");
    this.conn.installSessionKey(key, peerDeviceId);
  }

  /** Attach a transport + control client without the pairing handshake. */
  attachConnection(transport: RelayTransport, control?: ControlClient): ControlClient {
    this.conn = new RelayConnection(transport);
    this.control = control ?? new ControlClient(this.conn);
    this.permissions.setControlClient(this.control);
    return this.control;
  }

  /** Start (or restart) the receive loop using the current connection. */
  startReceiveLoop(): Promise<void> {
    const generation = ++this.connectionGeneration;
    this.stopLoop = false;
    this.loopPromise = this.runLoop(generation).catch(() => {});
    return this.loopPromise;
  }

  /** Disconnect: stop the loop, clear the session, drop the session key. */
  async disconnect(): Promise<void> {
    this.stopLoop = true;
    this.stopHeartbeat();
    try {
      await this.conn?.close();
    } catch {
      // ignore
    }
    this.conn = null;
    this.control = null;
    this.sessionStore.clear();
    this.connState.reset();
  }

  private controlClient(): ControlClient {
    if (!this.control) {
      throw new Error("not connected: pair with a PC first");
    }
    return this.control;
  }
}

export type { SecretStore };
// end of file
