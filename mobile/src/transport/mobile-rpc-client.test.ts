import { describe, expect, it } from "vitest";
import nacl from "tweetnacl";

import { authenticationPayload, pairingPayload } from "../protocol";
import {
  PAIRED_HOSTS_SECURE_STORE_KEY,
  PairedHostStore,
  type PairedHost,
} from "../storage/paired-host-store";
import { MobileRpcClient, type MobileWebSocket } from "./mobile-rpc-client";

function encode(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
}

function uuid(value: number): string {
  return `00000000-0000-0000-0000-${value.toString().padStart(12, "0")}`;
}

const offer = {
  offerId: uuid(1),
  endpoint: "ws://100.88.4.2:6769",
  protocolVersion: 1,
  hostPublicKey: "",
  pairingSecret: "pairing-secret",
  expiresAt: "2099-09-12T12:05:00Z",
};

class FakeSocket implements MobileWebSocket {
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onerror: (() => void) | null = null;
  onclose: (() => void) | null = null;
  readonly sent: string[] = [];
  closed = false;

  send(value: string): void {
    this.sent.push(value);
  }

  close(): void {
    this.closed = true;
    this.onclose?.();
  }

  open(): void {
    this.onopen?.();
  }

  receive(value: unknown): void {
    this.onmessage?.({ data: JSON.stringify(value) });
  }

  closeWithoutWebSocketClose(): void {
    this.closed = true;
  }
}

function statusResult(minimumCompatibleMobileVersion = 1) {
  return {
    host_name: "Zed Desktop",
    protocol_version: 1,
    minimum_compatible_mobile_version: minimumCompatibleMobileVersion,
    capabilities: ["status_read"],
  };
}

function hostFor(serverKeys: nacl.SignKeyPair, clientKeys: nacl.SignKeyPair): PairedHost {
  return {
    id: uuid(2),
    endpoint: offer.endpoint,
    hostPublicKey: encode(serverKeys.publicKey),
    grantId: uuid(2),
    token: "opaque-token",
    deviceSecretKey: encode(clientKeys.secretKey),
    displayName: "Host",
  };
}

describe("MobileRpcClient", () => {
  it("pins the server key before pairing and authenticating", async () => {
    const serverKeys = nacl.sign.keyPair();
    const clientKeys = nacl.sign.keyPair();
    const pinnedOffer = { ...offer, hostPublicKey: encode(serverKeys.publicKey) };
    const socket = new FakeSocket();
    const client = new MobileRpcClient({ webSocketFactory: () => socket });
    const pairing = client.pair(pinnedOffer, "Phone", { socket, clientKeys });

    socket.open();
    const begin = JSON.parse(socket.sent[0]);
    const serverNonce = encode(nacl.randomBytes(24));
    const signature = nacl.sign.detached(
      pairingPayload(1, pinnedOffer.offerId, begin.client_nonce, serverNonce),
      serverKeys.secretKey,
    );
    socket.receive({
      type: "pair_challenge",
      offer_id: pinnedOffer.offerId,
      client_nonce: begin.client_nonce,
      server_nonce: serverNonce,
      host_signature: encode(signature),
    });
    expect(socket.sent.map((frame) => JSON.parse(frame).type)).toEqual(["pair_begin", "pair_complete"]);

    socket.receive({ type: "pair_complete", grant_id: uuid(2), token: "opaque-token" });
    socket.receive({ type: "authenticated", grant_id: uuid(2) });
    await expect(pairing).resolves.toMatchObject({ grantId: uuid(2) });
    expect(client.status()).toBe("connected");
    client.close();
  });

  it("does not send pairing secrets after a bad server signature", async () => {
    const serverKeys = nacl.sign.keyPair();
    const wrongKeys = nacl.sign.keyPair();
    const pinnedOffer = { ...offer, hostPublicKey: encode(serverKeys.publicKey) };
    const socket = new FakeSocket();
    const client = new MobileRpcClient({ webSocketFactory: () => socket });
    const pairing = client.pair(pinnedOffer, "Phone");

    socket.open();
    const begin = JSON.parse(socket.sent[0]);
    const serverNonce = encode(nacl.randomBytes(24));
    const signature = nacl.sign.detached(
      pairingPayload(1, pinnedOffer.offerId, begin.client_nonce, serverNonce),
      wrongKeys.secretKey,
    );
    socket.receive({
      type: "pair_challenge",
      offer_id: pinnedOffer.offerId,
      client_nonce: begin.client_nonce,
      server_nonce: serverNonce,
      host_signature: encode(signature),
    });

    await expect(pairing).rejects.toThrow();
    expect(socket.sent.map((frame) => JSON.parse(frame).type)).toEqual(["pair_begin"]);
  });

  it("blocks incompatible status before dispatching another domain RPC", async () => {
    const serverKeys = nacl.sign.keyPair();
    const clientKeys = nacl.sign.keyPair();
    const host = hostFor(serverKeys, clientKeys);
    const socket = new FakeSocket();
    const tokenDigest = new Uint8Array(32).fill(7);
    const client = new MobileRpcClient({
      webSocketFactory: () => socket,
      digest: async () => tokenDigest,
    });
    const connection = client.connect(host);

    socket.open();
    const connect = JSON.parse(socket.sent[0]);
    const serverNonce = encode(nacl.randomBytes(24));
    const payload = authenticationPayload(1, host.grantId, connect.client_nonce, serverNonce, tokenDigest);
    socket.receive({
      type: "server_challenge",
      grant_id: host.grantId,
      client_nonce: connect.client_nonce,
      server_nonce: serverNonce,
      host_signature: encode(nacl.sign.detached(payload, serverKeys.secretKey)),
    });
    await Promise.resolve();
    socket.receive({ type: "authenticated", grant_id: host.grantId });
    const statusRequest = JSON.parse(socket.sent[2]);
    socket.receive({
      type: "response",
      request_id: statusRequest.request_id,
      operation_id: null,
      result: statusResult(2),
    });

    await expect(connection).rejects.toThrow("incompatible protocol version");
    expect(client.status()).toBe("incompatible");
    expect(socket.sent.map((frame) => JSON.parse(frame).type)).toEqual([
      "connect",
      "authenticate",
      "request",
    ]);
    client.close();
  });

  it("deletes a host on a matching grant_revoked event", async () => {
    const serverKeys = nacl.sign.keyPair();
    const clientKeys = nacl.sign.keyPair();
    const host = hostFor(serverKeys, clientKeys);
    const values = new Map<string, string>();
    const store = new PairedHostStore({
      async getItemAsync(key) {
        return values.get(key) ?? null;
      },
      async setItemAsync(key, value) {
        values.set(key, value);
      },
      async deleteItemAsync(key) {
        values.delete(key);
      },
    });
    await store.save(host);
    const socket = new FakeSocket();
    const tokenDigest = new Uint8Array(32).fill(3);
    const client = new MobileRpcClient({
      webSocketFactory: () => socket,
      hostStore: store,
      digest: async () => tokenDigest,
    });
    const connection = client.connect(host);
    socket.open();
    const connect = JSON.parse(socket.sent[0]);
    const serverNonce = encode(nacl.randomBytes(24));
    const payload = authenticationPayload(1, host.grantId, connect.client_nonce, serverNonce, tokenDigest);
    socket.receive({
      type: "server_challenge",
      grant_id: host.grantId,
      client_nonce: connect.client_nonce,
      server_nonce: serverNonce,
      host_signature: encode(nacl.sign.detached(payload, serverKeys.secretKey)),
    });
    await Promise.resolve();
    socket.receive({ type: "authenticated", grant_id: host.grantId });
    const statusRequest = JSON.parse(socket.sent[2]);
    socket.receive({
      type: "response",
      request_id: statusRequest.request_id,
      operation_id: null,
      result: statusResult(),
    });
    await connection;

    socket.receive({ type: "event", event: "grant_revoked", payload: { grant_id: host.grantId } });
    await Promise.resolve();
    expect(await store.list()).toEqual([]);
    expect(values.has(PAIRED_HOSTS_SECURE_STORE_KEY)).toBe(false);
    expect(client.status()).toBe("revoked");
  });

  it("uses foreground recovery after a socket disappears without close", async () => {
    const serverKeys = nacl.sign.keyPair();
    const clientKeys = nacl.sign.keyPair();
    const host = hostFor(serverKeys, clientKeys);
    const socket = new FakeSocket();
    const sockets = [socket, new FakeSocket()];
    const client = new MobileRpcClient({
      webSocketFactory: () => sockets.shift() ?? new FakeSocket(),
      digest: async () => new Uint8Array(32).fill(1),
    });
    const connection = client.connect(host);
    socket.open();
    const connect = JSON.parse(socket.sent[0]);
    const serverNonce = encode(nacl.randomBytes(24));
    const payload = authenticationPayload(1, host.grantId, connect.client_nonce, serverNonce, new Uint8Array(32).fill(1));
    socket.receive({
      type: "server_challenge",
      grant_id: host.grantId,
      client_nonce: connect.client_nonce,
      server_nonce: serverNonce,
      host_signature: encode(nacl.sign.detached(payload, serverKeys.secretKey)),
    });
    await Promise.resolve();
    socket.receive({ type: "authenticated", grant_id: host.grantId });
    const statusRequest = JSON.parse(socket.sent[2]);
    socket.receive({
      type: "response",
      request_id: statusRequest.request_id,
      operation_id: null,
      result: statusResult(),
    });
    await connection;
    expect(client.status()).toBe("connected");

    socket.closeWithoutWebSocketClose();
    client.notifyForeground();
    expect(client.status()).toBe("reconnecting");
    client.close();
  });
});
