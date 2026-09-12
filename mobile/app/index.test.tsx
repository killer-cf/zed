// @ts-expect-error React does not ship declarations in this package.
import * as React from "react";
import { act, create, type ReactTestRenderer } from "react-test-renderer";
import { describe, expect, it, vi } from "vitest";

vi.mock("react-native", () => ({
  Pressable: "Pressable",
  ScrollView: "ScrollView",
  StyleSheet: { create: (styles: unknown) => styles },
  Text: "Text",
  View: "View",
}));
const focusState = vi.hoisted(() => ({ focused: true }));
vi.mock("expo-router", () => ({
  useRouter: () => ({
    push: vi.fn(),
  }),
  useIsFocused: () => focusState.focused,
}));
import type { Status } from "../src/protocol";
import { HostDashboard } from "./index";

void React;

const host = {
  id: "00000000-0000-0000-0000-000000000001",
  endpoint: "ws://100.88.4.2:6769",
  hostPublicKey: "host-key",
  grantId: "00000000-0000-0000-0000-000000000002",
  token: "token",
  deviceSecretKey: "device-key",
  displayName: "Office Zed",
};

const status: Status = {
  host_name: "Office Zed",
  protocol_version: 1,
  minimum_compatible_mobile_version: 1,
  capabilities: ["status_read"],
};

type Store = {
  list: () => Promise<Array<typeof host>>;
  save: (value: typeof host) => Promise<void>;
  remove: (id: string) => Promise<void>;
};

function storeWith(hosts: Array<typeof host>): Store {
  return {
    list: async () => hosts,
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

describe("host dashboard", () => {
  it("renders an honest empty state without feature cards", async () => {
    let renderer!: ReactTestRenderer;
    await act(async () => {
      renderer = create(
        <HostDashboard
          store={storeWith([])}
          createClient={() => {
            throw new Error("not called");
          }}
        />,
      );
      await Promise.resolve();
    });

    expect(hasText(renderer, "No paired hosts")).toBe(true);
    expect(hasText(renderer, "worktree")).toBe(false);
    expect(hasText(renderer, "thread")).toBe(false);
    expect(hasText(renderer, "account")).toBe(false);
  });

  it("shows host endpoint and live connection state with retry and remove actions", async () => {
    const client = {
      connect: vi.fn(async () => status),
      retry: vi.fn(),
      close: vi.fn(),
      status: vi.fn(() => "connected" as const),
    };
    const remove = vi.fn(async () => undefined);
    const store: Store = { ...storeWith([host]), remove };
    let renderer!: ReactTestRenderer;
    await act(async () => {
      renderer = create(<HostDashboard store={store} createClient={() => client} />);
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(hasText(renderer, host.endpoint)).toBe(true);
    expect(hasText(renderer, "connected")).toBe(true);
    await act(async () => press(renderer, "Retry"));
    expect(client.retry).toHaveBeenCalledTimes(1);
    await act(async () => press(renderer, "Remove"));
    expect(remove).toHaveBeenCalledWith(host.id);
  });
  it("reloads profiles and owns fresh clients when returning to the dashboard", async () => {
    focusState.focused = true;
    const firstClient = {
      connect: vi.fn(async () => status),
      retry: vi.fn(),
      close: vi.fn(),
      status: vi.fn(() => "connected" as const),
    };
    const secondClient = {
      connect: vi.fn(async () => status),
      retry: vi.fn(),
      close: vi.fn(),
      status: vi.fn(() => "connected" as const),
    };
    const list = vi.fn().mockResolvedValueOnce([host]).mockResolvedValueOnce([host]);
    const store: Store = { ...storeWith([]), list };
    const createClient = vi.fn().mockReturnValueOnce(firstClient).mockReturnValueOnce(secondClient);
    let renderer!: ReactTestRenderer;
    await act(async () => {
      renderer = create(<HostDashboard store={store} createClient={createClient} />);
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(list).toHaveBeenCalledTimes(1);

    focusState.focused = false;
    await act(async () => {
      renderer.update(<HostDashboard store={store} createClient={createClient} />);
    });
    expect(firstClient.close).toHaveBeenCalledTimes(1);

    focusState.focused = true;
    await act(async () => {
      renderer.update(<HostDashboard store={store} createClient={createClient} />);
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(list).toHaveBeenCalledTimes(2);
    expect(createClient).toHaveBeenCalledTimes(2);
    renderer.unmount();
  });
  it("clears a transient connect error when recovery reports reconnecting", async () => {
    focusState.focused = true;
    vi.useFakeTimers();
    let currentState: "unreachable" | "reconnecting" | "connected" = "unreachable";
    const client = {
      connect: vi.fn(async () => {
        throw new Error("offline");
      }),
      retry: vi.fn(),
      close: vi.fn(),
      status: vi.fn(() => currentState),
    };
    const store = storeWith([host]);
    let renderer!: ReactTestRenderer;
    try {
      await act(async () => {
        renderer = create(<HostDashboard store={store} createClient={() => client} />);
        await Promise.resolve();
        await Promise.resolve();
      });
      expect(hasText(renderer, "offline")).toBe(true);

      currentState = "reconnecting";
      await act(async () => {
        vi.advanceTimersByTime(500);
        await Promise.resolve();
      });
      expect(hasText(renderer, "offline")).toBe(false);
    } finally {
      renderer?.unmount();
      vi.useRealTimers();
    }
  });
});
