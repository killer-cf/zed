import { describe, expect, it } from "vitest";

import { decodePairingInput, parsePairingCode, parsePairingLink } from "./pairing-code";

const code =
  "eyJvZmZlcl9pZCI6IjAxOGYwYWE2LThkM2ItN2NjZi05ZmUwLWNkMjNmYjQwYWQ0MCIsImVuZHBvaW50Ijoid3M6Ly8xMDAuODguNC4yOjY3NjkiLCJwcm90b2NvbF92ZXJzaW9uIjoxLCJob3N0X3B1YmxpY19rZXkiOiJiYXNlNjR1cmwta2V5IiwicGFpcmluZ19zZWNyZXQiOiJiYXNlNjR1cmwtc2VjcmV0IiwiZXhwaXJlc19hdCI6IjIwMjYtMDktMTJUMTI6MDU6MDBaIn0";
const url = `zed-mobile://pair?code=${code}`;

describe("pairing input parser", () => {
  it("uses one decoder for QR, paste, and deep-link values", () => {
    expect(decodePairingInput(`  ${url}  `)).toEqual(parsePairingCode(url));
    expect(parsePairingLink(url)).toEqual(decodePairingInput(url));
  });

  it("rejects values that are not canonical pairing URLs", () => {
    expect(() => decodePairingInput("https://example.test/pair?code=secret")).toThrow(
      "invalid pairing URL",
    );
  });
});
