// @ts-expect-error React does not ship declarations in this package.
import * as React from "react";
import { act, create, type ReactTestRenderer } from "react-test-renderer";
import { describe, expect, it, vi } from "vitest";

vi.mock("react-native", () => ({
  Pressable: "Pressable",
  StyleSheet: { create: (styles: unknown) => styles },
  Text: "Text",
  TextInput: "TextInput",
  View: "View",
}));
const { requestCameraPermissionsAsync } = vi.hoisted(() => ({
  requestCameraPermissionsAsync: vi.fn(async () => ({ granted: false })),
}));
vi.mock("expo-camera", () => ({
  Camera: { requestCameraPermissionsAsync },
  CameraView: "CameraView",
}));
vi.mock("expo-clipboard", () => ({ getStringAsync: vi.fn(async () => "") }));
vi.mock("expo-linking", () => ({
  addEventListener: vi.fn(() => ({ remove: vi.fn() })),
}));
vi.mock("expo-router", () => ({
  useRouter: () => ({ replace: vi.fn(), back: vi.fn() }),
  useLocalSearchParams: () => ({}),
}));
void React;

import { encodePairingUrl, type PairingOffer } from "../src/protocol";
import { PairScreen } from "./pair";

const offer: PairingOffer = {
  offerId: "00000000-0000-0000-0000-000000000001",
  endpoint: "ws://100.88.4.2:6769",
  protocolVersion: 1,
  hostPublicKey: "host-key",
  pairingSecret: "pairing-secret",
  expiresAt: "2099-09-12T12:05:00Z",
};
const pairingUrl = encodePairingUrl(offer);
const profile = {
  id: "00000000-0000-0000-0000-000000000002",
  endpoint: offer.endpoint,
  hostPublicKey: offer.hostPublicKey,
  grantId: "00000000-0000-0000-0000-000000000002",
  token: "token",
  deviceSecretKey: "device-key",
  displayName: "This phone",
};

type Store = {
  list: () => Promise<never[]>;
  save: (value: typeof profile) => Promise<void>;
  remove: (id: string) => Promise<void>;
};

function store(): Store {
  return {
    list: async () => [],
    save: async () => undefined,
    remove: async () => undefined,
  };
}

function hasText(renderer: ReactTestRenderer, expected: string): boolean {
  return renderer.root.findAll((node) => String(node.type) === "Text").some((node) => {
    const children = node.props.children;
    return typeof children === "string" && children.includes(expected);
  });
}

function press(renderer: ReactTestRenderer, expected: string): void {
  const button = renderer.root.findAll((node) => String(node.type) === "Pressable").find((node) => {
    const child = node.props.children;
    return child?.props?.children === expected;
  });
  if (!button) throw new Error(`button not found: ${expected}`);
  button.props.onPress();
}

describe("pair screen", () => {
  it("does not request camera permission until scan is selected and offers paste when denied", async () => {
    let renderer!: ReactTestRenderer;
    await act(async () => {
      renderer = create(<PairScreen store={store()} pair={async () => profile} />);
    });

    expect(requestCameraPermissionsAsync).not.toHaveBeenCalled();
    await act(async () => press(renderer, "Scan QR code"));
    expect(requestCameraPermissionsAsync).toHaveBeenCalledTimes(1);
    expect(hasText(renderer, "Camera permission was denied")).toBe(true);
    expect(hasText(renderer, "Paste pairing link")).toBe(true);
  });

  it("passes pasted links through the pairing decoder and prevents duplicate pairing", async () => {
    const save = vi.fn(async () => undefined);
    const pair = vi.fn(async () => profile);
    let renderer!: ReactTestRenderer;
    await act(async () => {
      renderer = create(<PairScreen store={{ ...store(), save }} pair={pair} />);
    });
    await act(async () => press(renderer, "Paste pairing link"));
    const input = renderer.root.findAll((node) => String(node.type) === "TextInput")[0];
    if (!input) throw new Error("input not found");
    await act(async () => {
      input.props.onChangeText(pairingUrl);
    });
    await act(async () => {
      press(renderer, "Pair host");
      press(renderer, "Pair host");
      await Promise.resolve();
    });

    expect(pair).toHaveBeenCalledTimes(1);
    expect(save).toHaveBeenCalledWith(profile);
  });
  it("keeps a completed profile recoverable when the first SecureStore write fails", async () => {
    const save = vi
      .fn<Store["save"]>()
      .mockRejectedValueOnce(new Error("SecureStore unavailable"))
      .mockResolvedValue(undefined);
    const pair = vi.fn(async () => profile);
    let renderer!: ReactTestRenderer;
    await act(async () => {
      renderer = create(<PairScreen store={{ ...store(), save }} pair={pair} />);
    });
    await act(async () => press(renderer, "Paste pairing link"));
    const input = renderer.root.findAll((node) => String(node.type) === "TextInput")[0];
    if (!input) throw new Error("input not found");
    await act(async () => {
      input.props.onChangeText(pairingUrl);
    });
    await act(async () => {
      press(renderer, "Pair host");
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(pair).toHaveBeenCalledTimes(1);
    expect(save).toHaveBeenCalledTimes(1);
    expect(hasText(renderer, "Retry saving host")).toBe(true);

    await act(async () => {
      press(renderer, "Retry saving host");
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(pair).toHaveBeenCalledTimes(1);
    expect(save).toHaveBeenCalledTimes(2);
  });
});
