import nacl from "tweetnacl";

import {
  authenticationPayload,
  minimumCompatibleDesktopVersion,
  pairingPayload,
  parseServerFrame,
  parseStatus,
  protocolVersion,
  type PairingOffer,
  type ServerFrame,
  type Status,
} from "../protocol";
import {
  PairedHostStore,
  type PairedHost,
  type PairedHostStoreLike,
  type SecureStoreLike,
} from "../storage/paired-host-store";

export type ConnectionState =
  | "disconnected"
  | "pairing"
  | "connecting"
  | "connected"
  | "reconnecting"
  | "unreachable"
  | "revoked"
  | "incompatible"
  | "key_mismatch";

export interface MobileWebSocket {
  onopen: (() => void) | null;
  onmessage: ((event: { data: unknown }) => void) | null;
  onerror: (() => void) | null;
  onclose: (() => void) | null;
  send(value: string): void;
  close(code?: number, reason?: string): void;
  readonly readyState?: number;
}
export type WebSocketFactory = (endpoint: string) => MobileWebSocket;

export type NetworkSubscription = { remove(): void };

export type NetworkListener = {
  addNetworkStateListener(listener: (state: { isConnected?: boolean; isInternetReachable?: boolean }) => void): NetworkSubscription;
};
export type AppStateSubscription = { remove(): void };
export type AppStateListener = {
  addEventListener(event: "change", listener: (state: string) => void): AppStateSubscription;
};
export type MobileRpcClientOptions = {
  webSocketFactory?: WebSocketFactory;
  createWebSocket?: WebSocketFactory;
  webSocket?: WebSocketFactory;
  hostStore?: PairedHostStoreLike;
  pairedHostStore?: PairedHostStoreLike;
  store?: PairedHostStoreLike;
  secureStore?: SecureStoreLike;
  network?: NetworkListener;
  networkListener?: NetworkListener;
  appState?: AppStateListener;
  appStateListener?: AppStateListener;
  randomBytes?: (length: number) => Uint8Array;
  digest?: (value: Uint8Array) => Promise<Uint8Array> | Uint8Array;
  heartbeatIntervalMs?: number;
  heartbeatTimeoutMs?: number;
  reconnectDelaysMs?: readonly number[];
  maxReconnectAttempts?: number;
};
export type PairOptions = {
  socket?: MobileWebSocket;
  clientKeys?: nacl.SignKeyPair;
};

type PendingStatus = {
  requestId: string;
  resolve(status: Status): void;
  reject(error: Error): void;
};

type PairingContext = {
  offer: PairingOffer;
  label: string;
  clientKeys: nacl.SignKeyPair;
  clientNonce: string;
  serverNonce?: string;
  grantId?: string;
  token?: string;
  resolve(profile: PairedHost): void;
  reject(error: Error): void;
};

const DEFAULT_HEARTBEAT_INTERVAL_MS = 15_000;
const DEFAULT_HEARTBEAT_TIMEOUT_MS = 10_000;
const DEFAULT_RECONNECT_DELAYS_MS = [500, 1_000, 2_000, 4_000, 8_000, 15_000];
const DEFAULT_MAX_RECONNECT_ATTEMPTS = DEFAULT_RECONNECT_DELAYS_MS.length;
const OPEN_READY_STATE = 1;
const FOREGROUND_STATES = new Set(["active", "unknown"]);

const defaultWebSocketFactory: WebSocketFactory = (endpoint) => {
  const Socket = globalThis.WebSocket;
  if (!Socket) throw new Error("WebSocket is unavailable");
  return new Socket(endpoint) as unknown as MobileWebSocket;
};

export class MobileRpcClient {
  private readonly webSocketFactory: WebSocketFactory;
  private readonly hostStore?: PairedHostStoreLike;
  private readonly network?: NetworkListener;
  private readonly appState?: AppStateListener;
  private readonly randomBytes: (length: number) => Uint8Array;
  private readonly digest: (value: Uint8Array) => Promise<Uint8Array> | Uint8Array;
  private readonly heartbeatIntervalMs: number;
  private readonly heartbeatTimeoutMs: number;
  private readonly reconnectDelaysMs: readonly number[];
  private readonly maxReconnectAttempts: number;

  private connectionState: ConnectionState = "disconnected";
  private socket: MobileWebSocket | undefined;
  private host: PairedHost | undefined;
  private currentStatus: Status | undefined;
  private pendingStatus: PendingStatus | undefined;
  private pairing: PairingContext | undefined;
  private pairingSocketOverride: MobileWebSocket | undefined;
  private intentionalClose = false;
  private generation = 0;
  private reconnectAttempt = 0;
  private reconnectTimer: ReturnType<typeof setTimeout> | undefined;
  private heartbeatTimer: ReturnType<typeof setInterval> | undefined;
  private heartbeatTimeoutTimer: ReturnType<typeof setTimeout> | undefined;
  private pendingHeartbeatNonce: string | undefined;
  private networkSubscription: NetworkSubscription | undefined;
  private appStateSubscription: AppStateSubscription | undefined;
  private recoveryListenersStarted = false;

  constructor(options: MobileRpcClientOptions = {}) {
    this.webSocketFactory =
      options.webSocketFactory ?? options.createWebSocket ?? options.webSocket ?? defaultWebSocketFactory;
    this.hostStore =
      options.hostStore ??
      options.pairedHostStore ??
      options.store ??
      (options.secureStore ? new PairedHostStore(options.secureStore) : undefined);
    this.network = options.network ?? options.networkListener;
    this.appState = options.appState ?? options.appStateListener;
    this.randomBytes = options.randomBytes ?? ((length) => nacl.randomBytes(length));
    this.digest = options.digest ?? sha256Bytes;
    this.heartbeatIntervalMs = options.heartbeatIntervalMs ?? DEFAULT_HEARTBEAT_INTERVAL_MS;
    this.heartbeatTimeoutMs = options.heartbeatTimeoutMs ?? DEFAULT_HEARTBEAT_TIMEOUT_MS;
    this.reconnectDelaysMs = options.reconnectDelaysMs ?? DEFAULT_RECONNECT_DELAYS_MS;
    this.maxReconnectAttempts = options.maxReconnectAttempts ?? DEFAULT_MAX_RECONNECT_ATTEMPTS;
  }

  static async pair(
    offer: PairingOffer,
    label: string,
    options: MobileRpcClientOptions & PairOptions = {},
  ): Promise<PairedHost> {
    const client = new MobileRpcClient(options);
    try {
      return await client.pair(offer, label, options);
    } finally {
      client.close();
    }
  }

  async pair(offer: PairingOffer, label: string, options: PairOptions = {}): Promise<PairedHost> {
    this.closeSocket();
    this.clearRecoveryTimers();
    this.intentionalClose = false;
    this.connectionState = "pairing";
    this.currentStatus = undefined;

    validatePairingOffer(offer);
    validateDeviceLabel(label);
    if (offer.protocolVersion !== protocolVersion || isExpired(offer.expiresAt)) {
      this.connectionState = offer.protocolVersion !== protocolVersion ? "incompatible" : "unreachable";
      throw new Error(offer.protocolVersion !== protocolVersion ? "incompatible protocol version" : "pairing offer expired");
    }

    const clientKeys = options.clientKeys ?? nacl.sign.keyPair();
    const clientNonce = encodeBase64Url(this.randomBytes(24));
    return this.startPairing(offer, label, clientKeys, clientNonce, options.socket);
  }

  status(): ConnectionState {
    return this.connectionState;
  }

  async connect(host: PairedHost): Promise<Status> {
    this.closeSocket();
    this.clearRecoveryTimers();
    this.intentionalClose = false;
    this.host = { ...host };
    this.currentStatus = undefined;
    this.pendingStatus = undefined;
    this.reconnectAttempt = 0;

    try {
      validatePairedHost(host);
    } catch (error) {
      this.connectionState = "key_mismatch";
      throw asError(error, "invalid paired host");
    }

    this.connectionState = "connecting";
    this.startRecoveryListeners();
    return new Promise<Status>((resolve, reject) => {
      this.pendingStatus = {
        requestId: makeUuid(this.randomBytes),
        resolve,
        reject,
      };
      this.openSocket(host.endpoint);
    });
  }

  retry(): void {
    this.forceReconnect();
  }

  notifyForeground(): void {
    this.forceReconnect();
  }

  close(): void {
    this.intentionalClose = true;
    this.clearRecoveryTimers();
    this.stopRecoveryListeners();
    this.closeSocket();
    const error = new Error("mobile RPC client closed");
    this.pendingStatus?.reject(error);
    this.pendingStatus = undefined;
    const wasPairing = this.pairing !== undefined;
    this.pairing?.reject(error);
    this.pairing = undefined;
    if (wasPairing) this.host = undefined;
    this.currentStatus = undefined;
    this.connectionState = "disconnected";
  }

  private async startPairing(
    offer: PairingOffer,
    label: string,
    clientKeys: nacl.SignKeyPair,
    clientNonce: string,
    socketOverride?: MobileWebSocket,
  ): Promise<PairedHost> {
    return new Promise<PairedHost>((resolve, reject) => {
      this.pairing = {
        offer,
        label,
        clientKeys,
        clientNonce,
        resolve,
        reject,
      };
      this.pairingSocketOverride = socketOverride;
      this.startRecoveryListeners();
      this.openSocket(offer.endpoint);
    });
  }

  private openSocket(endpoint: string): void {
    const generation = ++this.generation;
    let socket: MobileWebSocket;
    try {
      socket = this.pairingSocketOverride ?? this.webSocketFactory(endpoint);
    } catch (error) {
      this.handleSocketFailure(generation, asError(error, "WebSocket unavailable"));
      return;
    }
    this.pairingSocketOverride = undefined;
    this.socket = socket;
    socket.onopen = () => {
      if (!this.isCurrentSocket(socket, generation)) return;
      if (this.pairing) {
        this.sendPairBegin(socket);
      } else if (this.host) {
        this.sendConnect(socket);
      }
    };
    socket.onmessage = (event) => {
      if (this.isCurrentSocket(socket, generation)) void this.handleMessage(event.data);
    };
    socket.onerror = () => {
      if (this.isCurrentSocket(socket, generation)) this.handleSocketFailure(generation, new Error("WebSocket error"));
    };
    socket.onclose = () => {
      if (this.isCurrentSocket(socket, generation)) this.handleSocketFailure(generation, new Error("WebSocket closed"));
    };

    if (socket.readyState === OPEN_READY_STATE) socket.onopen();
  }

  private sendPairBegin(socket: MobileWebSocket): void {
    const pairing = this.pairing;
    if (!pairing) return;
    this.send(socket, {
      type: "pair_begin",
      offer_id: pairing.offer.offerId,
      client_nonce: pairing.clientNonce,
    });
  }

  private sendConnect(socket: MobileWebSocket): void {
    const host = this.host;
    if (!host) return;
    const clientNonce = encodeBase64Url(this.randomBytes(24));
    this.lastSentConnectFrame = { client_nonce: clientNonce };
    this.authenticationClientNonce = clientNonce;
    this.authenticationChallengeReceived = false;
    this.send(socket, {
      type: "connect",
      grant_id: host.grantId,
      client_nonce: clientNonce,
    });
  }

  private lastSentConnectFrame: { client_nonce: string } | undefined;
  private authenticationChallengeReceived = false;
  private authenticationClientNonce: string | undefined;

  private send(socket: MobileWebSocket, frame: Record<string, unknown>): void {
    socket.send(JSON.stringify(frame));
  }

  private async handleMessage(data: unknown): Promise<void> {
    if (typeof data !== "string") {
      this.handleProtocolFailure(new Error("invalid server frame"));
      return;
    }

    let frame: ServerFrame;
    try {
      frame = parseServerFrame(JSON.parse(data) as unknown);
    } catch {
      this.handleProtocolFailure(new Error("invalid server frame"));
      return;
    }

    if (this.pairing) {
      await this.handlePairingFrame(frame);
      return;
    }
    await this.handleConnectionFrame(frame);
  }

  private async handlePairingFrame(frame: ServerFrame): Promise<void> {
    const pairing = this.pairing;
    const socket = this.socket;
    if (!pairing || !socket) return;

    if (frame.type === "pair_challenge") {
      if (frame.offer_id !== pairing.offer.offerId || frame.client_nonce !== pairing.clientNonce) {
        this.failPairing(new Error("pairing challenge does not match offer"), "key_mismatch");
        return;
      }
      let payload: Uint8Array;
      try {
        payload = pairingPayload(
          pairing.offer.protocolVersion,
          pairing.offer.offerId,
          frame.client_nonce,
          frame.server_nonce,
        );
      } catch {
        this.failPairing(new Error("pairing challenge contained an invalid nonce"), "key_mismatch");
        return;
      }
      if (!verifyHostSignature(pairing.offer.hostPublicKey, frame.host_signature, payload)) {
        this.failPairing(new Error("pairing server signature did not match pinned host key"), "key_mismatch");
        return;
      }
      pairing.serverNonce = frame.server_nonce;
      const clientProof = nacl.sign.detached(payload, pairing.clientKeys.secretKey);
      this.send(socket, {
        type: "pair_complete",
        offer_id: pairing.offer.offerId,
        pairing_secret: pairing.offer.pairingSecret,
        client_public_key: encodeBase64Url(pairing.clientKeys.publicKey),
        client_proof: encodeBase64Url(clientProof),
        device_label: pairing.label,
      });
      return;
    }

    if (frame.type === "pair_complete") {
      if (pairing.grantId !== undefined || pairing.offer.offerId.length === 0) return;
      pairing.grantId = frame.grant_id;
      pairing.token = frame.token;
      return;
    }

    if (frame.type === "authenticated") {
      if (pairing.grantId !== frame.grant_id || pairing.token === undefined) {
        this.failPairing(new Error("pairing authentication did not match grant"), "key_mismatch");
        return;
      }
      const profile: PairedHost = {
        id: frame.grant_id,
        endpoint: pairing.offer.endpoint,
        hostPublicKey: pairing.offer.hostPublicKey,
        grantId: frame.grant_id,
        token: pairing.token,
        deviceSecretKey: encodeBase64Url(pairing.clientKeys.secretKey),
        displayName: pairing.label,
      };
      const resolvePairing = pairing.resolve;
      try {
        if (this.hostStore) await this.hostStore.save(profile);
      } catch (error) {
        this.failPairing(asError(error, "could not persist paired host"), "unreachable");
        return;
      }
      this.pairing = undefined;
      this.host = profile;
      this.connectionState = "connected";
      this.startHeartbeat();
      this.reconnectAttempt = 0;
      resolvePairing(profile);
      return;
    }

    if (frame.type === "error") {
      this.failPairing(new Error(frame.message), frame.code === "unsupported_protocol" ? "incompatible" : "unreachable");
      return;
    }

    this.failPairing(new Error("unexpected pairing server frame"), "unreachable");
  }

  private async handleConnectionFrame(frame: ServerFrame): Promise<void> {
    if (frame.type === "server_challenge") {
      await this.handleServerChallenge(frame);
      return;
    }
    if (frame.type === "authenticated") {
      if (
        !this.host ||
        frame.grant_id !== this.host.grantId ||
        !this.socket ||
        !this.authenticationChallengeReceived
      ) {
        this.handleProtocolFailure(new Error("authentication grant did not match host"));
        return;
      }
      this.sendStatusRequest();
      return;
    }
    if (frame.type === "response") {
      if (!this.pendingStatus || frame.request_id !== this.pendingStatus.requestId) return;
      const pending = this.pendingStatus;
      this.pendingStatus = undefined;
      let status: Status;
      try {
        status = parseStatus(frame.result);
      } catch {
        pending.reject(new Error("invalid status response"));
        this.handleProtocolFailure(new Error("invalid status response"));
        return;
      }
      if (status.protocol_version !== protocolVersion || status.minimum_compatible_mobile_version > minimumCompatibleDesktopVersion) {
        this.connectionState = "incompatible";
        pending.reject(new Error("incompatible protocol version"));
        this.closeSocket();
        return;
      }
      this.currentStatus = status;
      this.connectionState = "connected";
      this.reconnectAttempt = 0;
      this.startHeartbeat();
      pending.resolve(status);
      return;
    }
    if (frame.type === "pong") {
      if (frame.nonce === this.pendingHeartbeatNonce) this.clearHeartbeatTimeout();
      return;
    }
    if (frame.type === "event") {
      await this.handleEvent(frame.event, frame.payload);
      return;
    }
    if (frame.type === "error") {
      if (frame.code === "grant_revoked") {
        await this.revokeCurrentHost();
      } else if (this.pendingStatus && frame.request_id === this.pendingStatus.requestId) {
        const pending = this.pendingStatus;
        this.pendingStatus = undefined;
        pending.reject(new Error(frame.message));
        this.handleSocketFailure(this.generation, new Error(frame.message));
      }
    }
  }

  private async handleServerChallenge(frame: Extract<ServerFrame, { type: "server_challenge" }>): Promise<void> {
    const host = this.host;
    const socket = this.socket;
    const generation = this.generation;
    if (!host || !socket) {
      this.handleProtocolFailure(new Error("authentication challenge arrived without a connection"));
      return;
    }
    if (frame.grant_id !== host.grantId || frame.client_nonce !== this.authenticationClientNonce) {
      this.handleKeyMismatch(new Error("authentication challenge does not match host"));
      return;
    }
    let tokenDigest: Uint8Array;
    try {
      tokenDigest = await this.digest(new TextEncoder().encode(host.token));
      if (
        !this.isCurrentSocket(socket, generation) ||
        this.connectionState === "revoked" ||
        this.connectionState === "incompatible" ||
        this.connectionState === "key_mismatch"
      ) {
        return;
      }
      if (tokenDigest.length !== 32) throw new Error("invalid token digest");
      const payload = authenticationPayload(
        protocolVersion,
        host.grantId,
        frame.client_nonce,
        frame.server_nonce,
        tokenDigest,
      );
      if (!verifyHostSignature(host.hostPublicKey, frame.host_signature, payload)) {
        this.handleKeyMismatch(new Error("authentication server signature did not match pinned host key"));
        return;
      }
      const clientKeys = keyPairFromPersistedSecret(host.deviceSecretKey);
      const clientProof = nacl.sign.detached(payload, clientKeys.secretKey);
      this.authenticationChallengeReceived = true;
      this.send(socket, {
        type: "authenticate",
        grant_id: host.grantId,
        token: host.token,
        client_proof: encodeBase64Url(clientProof),
      });
    } catch (error) {
      if (this.isCurrentSocket(socket, generation)) {
        this.handleKeyMismatch(asError(error, "authentication challenge was invalid"));
      }
    }
  }

  private sendStatusRequest(): void {
    const socket = this.socket;
    const pending = this.pendingStatus;
    if (!socket || !pending) return;
    this.send(socket, {
      type: "request",
      request_id: pending.requestId,
      operation_id: null,
      method: "status.get",
      params: {},
    });
  }

  private async handleEvent(event: string, payload: unknown): Promise<void> {
    if (event === "grant_revoked" && isCurrentGrantPayload(payload, this.host?.grantId)) {
      await this.revokeCurrentHost();
      return;
    }
    if (event !== "status") return;
    try {
      const status = parseStatus(payload);
      if (status.protocol_version !== protocolVersion || status.minimum_compatible_mobile_version > minimumCompatibleDesktopVersion) {
        this.connectionState = "incompatible";
        this.closeSocket();
        return;
      }
      this.currentStatus = status;
    } catch {
      // An unsolicited event cannot complete a request. Ignore malformed status data safely.
    }
  }

  private resolvePairing(profile: PairedHost): void {
    const pairing = this.pairing;
    if (pairing) pairing.resolve(profile);
  }

  private failPairing(error: Error, state: ConnectionState): void {
    const pairing = this.pairing;
    this.pairing = undefined;
    this.connectionState = state;
    this.closeSocket();
    pairing?.reject(error);
  }

  private handleKeyMismatch(error: Error): void {
    this.connectionState = "key_mismatch";
    this.closeSocket();
    this.pendingStatus?.reject(error);
    this.pendingStatus = undefined;
  }

  private handleProtocolFailure(error: Error): void {
    if (this.pairing) {
      this.failPairing(error, "unreachable");
    } else {
      this.handleSocketFailure(this.generation, error);
    }
  }

  private handleSocketFailure(generation: number, error: Error): void {
    if (generation !== this.generation || this.intentionalClose) return;
    this.clearHeartbeatTimers();
    this.closeSocket();
    if (this.pairing) {
      this.failPairing(error, "unreachable");
      return;
    }
    this.pendingStatus?.reject(error);
    this.pendingStatus = undefined;
    if (this.connectionState === "revoked" || this.connectionState === "incompatible" || this.connectionState === "key_mismatch") return;
    this.connectionState = "reconnecting";
    this.scheduleReconnect();
  }
  private forceReconnect(): void {
    if (!this.host || this.connectionState === "revoked" || this.connectionState === "incompatible" || this.connectionState === "key_mismatch") return;
    this.intentionalClose = false;
    this.reconnectAttempt = 0;
    this.clearReconnectTimer();
    this.clearHeartbeatTimers();
    this.closeSocket();
    this.connectionState = "reconnecting";
    this.startRecoveryListeners();
    if (!this.pendingStatus) {
      this.pendingStatus = {
        requestId: makeUuid(this.randomBytes),
        resolve: () => undefined,
        reject: () => undefined,
      };
    }
    this.openSocket(this.host.endpoint);
  }

  private scheduleReconnect(): void {
    if (!this.host || this.intentionalClose || this.connectionState === "revoked" || this.connectionState === "incompatible") return;
    if (this.reconnectAttempt >= this.maxReconnectAttempts) {
      this.connectionState = "unreachable";
      return;
    }
    const index = Math.min(this.reconnectAttempt, this.reconnectDelaysMs.length - 1);
    const delay = Math.max(0, this.reconnectDelaysMs[index] ?? 0);
    this.reconnectAttempt += 1;
    this.clearReconnectTimer();
    this.reconnectTimer = setTimeout(() => {
      this.reconnectTimer = undefined;
      if (!this.host || this.intentionalClose) return;
      this.connectionState = "reconnecting";
      this.pendingStatus = {
        requestId: makeUuid(this.randomBytes),
        resolve: () => undefined,
        reject: () => undefined,
      };
      this.openSocket(this.host.endpoint);
    }, delay);
  }

  private startHeartbeat(): void {
    this.clearHeartbeatTimers();
    if (!this.socket) return;
    this.heartbeatTimer = setInterval(() => this.sendHeartbeat(), this.heartbeatIntervalMs);
  }

  private sendHeartbeat(): void {
    const socket = this.socket;
    if (!socket || this.connectionState !== "connected") return;
    if (this.pendingHeartbeatNonce !== undefined) {
      this.handleSocketFailure(this.generation, new Error("heartbeat timed out"));
      return;
    }
    const nonce = encodeBase64Url(this.randomBytes(16));
    this.pendingHeartbeatNonce = nonce;
    this.send(socket, { type: "ping", nonce });
    this.heartbeatTimeoutTimer = setTimeout(() => {
      if (this.pendingHeartbeatNonce === nonce) {
        this.handleSocketFailure(this.generation, new Error("heartbeat timed out"));
      }
    }, this.heartbeatTimeoutMs);
  }

  private clearHeartbeatTimeout(): void {
    this.pendingHeartbeatNonce = undefined;
    if (this.heartbeatTimeoutTimer !== undefined) clearTimeout(this.heartbeatTimeoutTimer);
    this.heartbeatTimeoutTimer = undefined;
  }

  private clearHeartbeatTimers(): void {
    if (this.heartbeatTimer !== undefined) clearInterval(this.heartbeatTimer);
    this.heartbeatTimer = undefined;
    this.clearHeartbeatTimeout();
  }

  private clearRecoveryTimers(): void {
    this.clearReconnectTimer();
    this.clearHeartbeatTimers();
  }

  private clearReconnectTimer(): void {
    if (this.reconnectTimer !== undefined) clearTimeout(this.reconnectTimer);
    this.reconnectTimer = undefined;
  }

  private closeSocket(): void {
    const socket = this.socket;
    this.socket = undefined;
    this.generation += 1;
    if (!socket) return;
    socket.onopen = null;
    socket.onmessage = null;
    socket.onerror = null;
    socket.onclose = null;
    try {
      socket.close();
    } catch {
      // The socket is already unusable; recovery will create a fresh one.
    }
  }

  private isCurrentSocket(socket: MobileWebSocket, generation: number): boolean {
    return this.socket === socket && this.generation === generation;
  }

  private startRecoveryListeners(): void {
    if (this.recoveryListenersStarted) return;
    this.recoveryListenersStarted = true;
    const onNetwork = (state: { isConnected?: boolean; isInternetReachable?: boolean }) => {
      if (state.isConnected === true && state.isInternetReachable !== false) this.forceReconnect();
    };
    const onAppState = (state: string) => {
      if (FOREGROUND_STATES.has(state)) this.notifyForeground();
    };
    if (this.network) this.networkSubscription = this.network.addNetworkStateListener(onNetwork);
    if (this.appState) this.appStateSubscription = this.appState.addEventListener("change", onAppState);
    if (!this.network || !this.appState) void this.loadPlatformRecoveryListeners(onNetwork, onAppState);
  }

  // Expo modules are loaded lazily because SecureStore/Network/AppState have no native binding in
  // the node test runner. Native builds still resolve these imports on the first connection.
  private async loadPlatformRecoveryListeners(
    onNetwork: (state: { isConnected?: boolean; isInternetReachable?: boolean }) => void,
    onAppState: (state: string) => void,
  ): Promise<void> {
    if (!this.network) {
      try {
        const network = await import("expo-network");
        if (!this.recoveryListenersStarted) return;
        this.networkSubscription = network.addNetworkStateListener(onNetwork);
      } catch {
        // Network recovery is optional on runtimes without Expo's native module.
      }
    }
    if (!this.appState) {
      try {
        const native = await import("react-native");
        if (!this.recoveryListenersStarted) return;
        this.appStateSubscription = native.AppState.addEventListener("change", onAppState);
      } catch {
        // AppState recovery is optional in non-native test environments.
      }
    }
  }

  private stopRecoveryListeners(): void {
    this.recoveryListenersStarted = false;
    this.networkSubscription?.remove();
    this.networkSubscription = undefined;
    this.appStateSubscription?.remove();
    this.appStateSubscription = undefined;
  }
  private async revokeCurrentHost(): Promise<void> {
    const host = this.host;
    this.clearRecoveryTimers();
    this.intentionalClose = true;
    this.connectionState = "revoked";
    this.closeSocket();
    this.stopRecoveryListeners();
    if (host && this.hostStore) {
      try {
        await this.hostStore.remove(host.id);
      } catch {
        // Keep the revoked state even if the secure-store backend is temporarily unavailable.
      }
    }
    this.host = undefined;
    this.pendingStatus?.reject(new Error("paired host grant was revoked"));
    this.pendingStatus = undefined;
  }
}

function validatePairingOffer(offer: PairingOffer): void {
  if (!offer.offerId || !offer.endpoint || !offer.hostPublicKey || !offer.pairingSecret || !offer.expiresAt) {
    throw new Error("invalid pairing offer");
  }
  validateUuid(offer.offerId);
  validateWebSocketEndpoint(offer.endpoint);
  decodeFixed(offer.hostPublicKey, 32);
}

function validateDeviceLabel(label: string): void {
  const bytes = new TextEncoder().encode(label).length;
  if (bytes === 0 || bytes > 128) {
    throw new Error("device label must be between 1 and 128 bytes");
  }
}

function validatePairedHost(host: PairedHost): void {
  if (!host.id || !host.endpoint || !host.grantId || !host.token || !host.hostPublicKey || !host.deviceSecretKey) {
    throw new Error("invalid paired host");
  }
  validateUuid(host.id);
  validateUuid(host.grantId);
  validateWebSocketEndpoint(host.endpoint);
  decodeFixed(host.hostPublicKey, 32);
  decodeFixed(host.deviceSecretKey, 32, 64);
}

function validateUuid(value: string): void {
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(value)) {
    throw new Error("invalid UUID");
  }
}

function validateWebSocketEndpoint(endpoint: string): void {
  if (!/^wss?:\/\/[^/]+(?:\/.*)?$/i.test(endpoint)) throw new Error("invalid WebSocket endpoint");
}

function keyPairFromPersistedSecret(value: string): nacl.SignKeyPair {
  const secret = decodeFixed(value, 32, 64);
  if (secret.length === nacl.sign.secretKeyLength) return nacl.sign.keyPair.fromSecretKey(secret);
  return nacl.sign.keyPair.fromSeed(secret);
}

function verifyHostSignature(publicKey: string, signature: string, payload: Uint8Array): boolean {
  try {
    const key = decodeFixed(publicKey, 32);
    const sig = decodeFixed(signature, 64);
    return nacl.sign.detached.verify(payload, sig, key);
  } catch {
    return false;
  }
}

function decodeFixed(value: string, minimumLength: number, maximumLength = minimumLength): Uint8Array {
  if (!/^[A-Za-z0-9_-]*={0,2}$/.test(value)) throw new Error("invalid base64url data");
  const unpadded = value.replace(/=+$/, "");
  if (unpadded.length % 4 === 1) throw new Error("invalid base64url data");
  const suppliedPadding = value.length - unpadded.length;
  const requiredPadding = (4 - (unpadded.length % 4)) % 4;
  if (suppliedPadding !== 0 && suppliedPadding !== requiredPadding) throw new Error("invalid base64url data");
  const binary = atob(`${unpadded}${"=".repeat(requiredPadding)}`.replaceAll("-", "+").replaceAll("_", "/"));
  const bytes = Uint8Array.from(binary, (character) => character.charCodeAt(0));
  if (maximumLength === minimumLength) {
    if (bytes.length !== minimumLength) throw new Error("invalid byte length");
  } else if (bytes.length !== minimumLength && bytes.length !== maximumLength) {
    throw new Error("invalid byte length");
  }
  return bytes;
}

function encodeBase64Url(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
}

function makeUuid(randomBytes: (length: number) => Uint8Array): string {
  const bytes = randomBytes(16);
  bytes[6] = (bytes[6] & 0x0f) | 0x40;
  bytes[8] = (bytes[8] & 0x3f) | 0x80;
  const hex = encodeHex(bytes);
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

function encodeHex(bytes: Uint8Array): string {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

function isExpired(expiresAt: string): boolean {
  const timestamp = Date.parse(expiresAt);
  return !Number.isFinite(timestamp) || timestamp <= Date.now();
}

function isCurrentGrantPayload(payload: unknown, grantId: string | undefined): boolean {
  if (!grantId) return false;
  if (payload === null || payload === undefined) return true;
  if (typeof payload === "string") return payload === grantId;
  if (typeof payload !== "object") return false;
  const candidate = payload as Record<string, unknown>;
  const value = candidate.grant_id ?? candidate.grantId ?? candidate.id;
  return value === undefined || value === grantId;
}
function asError(error: unknown, fallback: string): Error {
  return error instanceof Error ? error : new Error(fallback);
}

async function sha256Bytes(value: Uint8Array): Promise<Uint8Array> {
  const subtle = globalThis.crypto?.subtle;
  if (subtle) return new Uint8Array(await subtle.digest("SHA-256", value as unknown as BufferSource));
  // Expo Crypto is loaded lazily for native runtimes where Web Crypto is unavailable.
  const crypto = await import("expo-crypto");
  const digest = await crypto.digest(crypto.CryptoDigestAlgorithm.SHA256, value as unknown as BufferSource);
  return new Uint8Array(digest);
}
