import { describe, expect, it } from "vitest";

import {
  authenticationPayload,
  decodePairingUrl,
  parseClientFrame,
  parsePairingOffer,
  parseServerFrame,
  parseStatus,
  pairingPayload,
  protocolVersion,
} from "./protocol";

const validUrl =
  "zed-mobile://pair?code=eyJvZmZlcl9pZCI6IjAxOGYwYWE2LThkM2ItN2NjZi05ZmUwLWNkMjNmYjQwYWQ0MCIsImVuZHBvaW50Ijoid3M6Ly8xMDAuODguNC4yOjY3NjkiLCJwcm90b2NvbF92ZXJzaW9uIjoxLCJob3N0X3B1YmxpY19rZXkiOiJiYXNlNjR1cmwta2V5IiwicGFpcmluZ19zZWNyZXQiOiJiYXNlNjR1cmwtc2VjcmV0IiwiZXhwaXJlc19hdCI6IjIwMjYtMDktMTJUMTI6MDU6MDBaIn0";

const offerJson = {
  offer_id: "018f0aa6-8d3b-7ccf-9fe0-cd23fb40ad40",
  endpoint: "ws://100.88.4.2:6769",
  protocol_version: 1,
  host_public_key: "base64url-key",
  pairing_secret: "base64url-secret",
  expires_at: "2026-09-12T12:05:00Z",
};

const requestId = "00000000-0000-0000-0000-000000000001";
const operationId = "00000000-0000-0000-0000-000000000002";

function hex(bytes: Uint8Array): string {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, "0")).join("");
}

describe("mobile protocol URL and DTO parsing", () => {
  it("decodes the canonical pairing URL into the mobile offer shape", () => {
    expect(decodePairingUrl(validUrl)).toEqual({
      offerId: "018f0aa6-8d3b-7ccf-9fe0-cd23fb40ad40",
      endpoint: "ws://100.88.4.2:6769",
      protocolVersion: 1,
      hostPublicKey: "base64url-key",
      pairingSecret: "base64url-secret",
      expiresAt: "2026-09-12T12:05:00Z",
    });
  });

  it("accepts padded base64url and rejects every non-Zed pairing URL shape", () => {
    expect(decodePairingUrl(`${validUrl}=`)).toEqual(decodePairingUrl(validUrl));
    expect(() => decodePairingUrl("orca://pair?code=abc")).toThrow("invalid pairing URL");
    expect(() => decodePairingUrl("zed-mobile://other?code=abc")).toThrow("invalid pairing URL");
    expect(() => decodePairingUrl("zed-mobile://pair?code=abc&extra=def")).toThrow(
      "invalid pairing URL",
    );
    expect(() => decodePairingUrl("zed-mobile://pair?code=abc&code=def")).toThrow(
      "invalid pairing URL",
    );
    expect(() => decodePairingUrl("zed-mobile://pair?value=abc")).toThrow("invalid pairing URL");
    expect(() => decodePairingUrl("zed-mobile://pair?code=not-json")).toThrow(
      "invalid pairing URL",
    );
  });

  it("parses the Rust pairing-offer JSON with strict fields and ISO expiry", () => {
    expect(parsePairingOffer(offerJson)).toEqual({
      offerId: offerJson.offer_id,
      endpoint: offerJson.endpoint,
      protocolVersion: offerJson.protocol_version,
      hostPublicKey: offerJson.host_public_key,
      pairingSecret: offerJson.pairing_secret,
      expiresAt: offerJson.expires_at,
    });
    expect(() => parsePairingOffer({ ...offerJson, unexpected: true })).toThrow();
    expect(() => parsePairingOffer({ ...offerJson, expires_at: "not-a-timestamp" })).toThrow();
  });
});

describe("mobile protocol frames and status", () => {
  it("preserves Rust frame discriminants, IDs, optional operation IDs, and params", () => {
    expect(
      parseClientFrame({
        type: "request",
        request_id: requestId,
        operation_id: operationId,
        method: "status.get",
        params: {},
      }),
    ).toEqual({
      type: "request",
      request_id: requestId,
      operation_id: operationId,
      method: "status.get",
      params: {},
    });
    expect(
      parseClientFrame({
        type: "request",
        request_id: requestId,
        operation_id: null,
        method: "status.get",
        params: {},
      }),
    ).toEqual({
      type: "request",
      request_id: requestId,
      operation_id: null,
      method: "status.get",
      params: {},
    });
    expect(() => parseClientFrame({ type: "request", method: "status.get", params: {} })).toThrow(
      "invalid client frame",
    );
    expect(() => parseServerFrame({ type: "response" })).toThrow("invalid server frame");
  });

  it("parses an authenticated response and keeps capabilities in Rust declaration order", () => {
    expect(
      parseServerFrame({
        type: "response",
        request_id: requestId,
        operation_id: operationId,
        result: {
          host_name: "Zed Desktop",
          protocol_version: protocolVersion,
          minimum_compatible_mobile_version: 1,
          capabilities: ["threads_read", "status_read"],
        },
      }),
    ).toEqual({
      type: "response",
      request_id: requestId,
      operation_id: operationId,
      result: {
        host_name: "Zed Desktop",
        protocol_version: 1,
        minimum_compatible_mobile_version: 1,
        capabilities: ["threads_read", "status_read"],
      },
    });
    expect(
      parseServerFrame({
        type: "authenticated",
        grant_id: "00000000-0000-0000-0000-000000000003",
      }),
    ).toEqual({
      type: "authenticated",
      grant_id: "00000000-0000-0000-0000-000000000003",
    });
    expect(
      parseStatus({
        host_name: "Zed Desktop",
        protocol_version: 1,
        minimum_compatible_mobile_version: 1,
        capabilities: ["threads_read", "status_read"],
      }),
    ).toEqual({
      host_name: "Zed Desktop",
      protocol_version: 1,
      minimum_compatible_mobile_version: 1,
      capabilities: ["status_read", "threads_read"],
    });
    expect(() => parseStatus({
      host_name: "Zed Desktop",
      protocol_version: 1,
      minimum_compatible_mobile_version: 1,
      capabilities: ["not_a_capability"],
    })).toThrow();
  });

  it("accepts every Rust client and server frame discriminant", () => {
    const clientFrames = [
      { type: "pair_begin", offer_id: requestId, client_nonce: "client" },
      {
        type: "pair_complete",
        offer_id: requestId,
        pairing_secret: "secret",
        client_public_key: "public-key",
        client_proof: "proof",
        device_label: "Phone",
      },
      { type: "connect", grant_id: requestId, client_nonce: "client" },
      { type: "authenticate", grant_id: requestId, token: "token", client_proof: "proof" },
      { type: "request", request_id: requestId, operation_id: null, method: "status.get", params: {} },
      { type: "ping", nonce: "nonce" },
    ];
    for (const frame of clientFrames) {
      expect(parseClientFrame(frame)).toEqual(frame);
    }

    const serverFrames = [
      {
        type: "pair_challenge",
        offer_id: requestId,
        client_nonce: "client",
        server_nonce: "server",
        host_signature: "signature",
      },
      { type: "pair_complete", grant_id: requestId, token: "token" },
      {
        type: "server_challenge",
        grant_id: requestId,
        client_nonce: "client",
        server_nonce: "server",
        host_signature: "signature",
      },
      { type: "authenticated", grant_id: requestId },
      { type: "response", request_id: requestId, operation_id: null, result: {} },
      { type: "event", event: "status", payload: {} },
      { type: "pong", nonce: "nonce" },
      { type: "error", request_id: null, code: "error", message: "message" },
    ];
    for (const frame of serverFrames) {
      expect(parseServerFrame(frame)).toEqual(frame);
    }
  });

  it("keeps Rust JSON field order for request and response envelopes", () => {
    const request = parseClientFrame({
      type: "request",
      request_id: requestId,
      operation_id: operationId,
      method: "status.get",
      params: {},
    });
    expect(JSON.stringify(request)).toBe(
      `{"type":"request","request_id":"${requestId}","operation_id":"${operationId}","method":"status.get","params":{}}`,
    );

    const response = parseServerFrame({
      type: "response",
      request_id: requestId,
      operation_id: operationId,
      result: {},
    });
    expect(JSON.stringify(response)).toBe(
      `{"type":"response","request_id":"${requestId}","operation_id":"${operationId}","result":{}}`,
    );
  });

  it("rejects unknown fields on every externally decoded envelope", () => {
    expect(() => parseClientFrame({ type: "ping", nonce: "nonce", unexpected: true })).toThrow();
    expect(() => parseServerFrame({
      type: "pong",
      nonce: "nonce",
      unexpected: true,
    })).toThrow();
    expect(() => parseServerFrame({
      type: "error",
      request_id: null,
      code: "error",
      message: "message",
      unexpected: true,
    })).toThrow();
  });
});

describe("mobile signing payloads", () => {
  it("matches the Rust pairing payload fixed vector byte-for-byte", () => {
    expect(
      hex(
        pairingPayload(
          protocolVersion,
          "018f0aa6-8d3b-7ccf-9fe0-cd23fb40ad40",
          "Y2xpZW50LW5vbmNl",
          "c2VydmVyLW5vbmNl",
        ),
      ),
    ).toBe(
      "7a65642d6d6f62696c652f706169722f76310001018f0aa68d3b7ccf9fe0cd23fb40ad400000000c636c69656e742d6e6f6e63650000000c7365727665722d6e6f6e6365",
    );
  });

  it("matches the Rust authentication payload fixed vector byte-for-byte", () => {
    expect(
      hex(
        authenticationPayload(
          protocolVersion,
          "7f6f3d5d-2f5b-4c8f-a1d8-123456789abc",
          "Y2xpZW50LW5vbmNl",
          "c2VydmVyLW5vbmNl",
          new Uint8Array([
            0x90, 0x65, 0x66, 0xf5, 0x95, 0xb9, 0x0e, 0x05, 0x45, 0x7a, 0x4e, 0x12, 0xb0, 0xe9,
            0x2d, 0x81, 0x18, 0x4f, 0xee, 0xcc, 0xe6, 0x4b, 0x64, 0x23, 0x23, 0x8a, 0x4e, 0xeb,
            0x25, 0xeb, 0xf5, 0x67,
          ]),
        ),
      ),
    ).toBe(
      "7a65642d6d6f62696c652f617574682f763100017f6f3d5d2f5b4c8fa1d8123456789abc0000000c636c69656e742d6e6f6e63650000000c7365727665722d6e6f6e636500000020906566f595b90e05457a4e12b0e92d81184feecce64b6423238a4eeb25ebf567",
    );
  });

  it("rejects malformed nonce encoding and a wrong digest length", () => {
    expect(() => pairingPayload(protocolVersion, "00000000-0000-0000-0000-000000000001", "not valid", "YQ")).toThrow();
    expect(() => authenticationPayload(
      protocolVersion,
      "00000000-0000-0000-0000-000000000001",
      "YQ",
      "not valid",
      new Uint8Array(31),
    )).toThrow();
  });
});
