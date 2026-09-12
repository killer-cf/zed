import { describe, expect, it } from "vitest";

import {
  PAIRED_HOSTS_SECURE_STORE_KEY,
  PairedHostStore,
  type PairedHost,
} from "./paired-host-store";
type Entry = { key: string; value: string };

class FakeSecureStore {
  readonly entries: Entry[] = [];

  async getItemAsync(key: string): Promise<string | null> {
    return this.entries.find((entry) => entry.key === key)?.value ?? null;
  }

  async setItemAsync(key: string, value: string): Promise<void> {
    const entry = this.entries.find((candidate) => candidate.key === key);
    if (entry) {
      entry.value = value;
    } else {
      this.entries.push({ key, value });
    }
  }

  async deleteItemAsync(key: string): Promise<void> {
    const index = this.entries.findIndex((entry) => entry.key === key);
    if (index >= 0) this.entries.splice(index, 1);
  }
}

const pairedHost: PairedHost = {
  id: "00000000-0000-0000-0000-000000000001",
  endpoint: "ws://100.88.4.2:6769",
  hostPublicKey: "host-public-key",
  grantId: "00000000-0000-0000-0000-000000000002",
  token: "opaque-token",
  deviceSecretKey: "device-secret-key",
  displayName: "Zed Desktop",
};

describe("PairedHostStore", () => {
  it("round-trips profiles through SecureStore and removes them", async () => {
    const secureStore = new FakeSecureStore();
    const hosts = new PairedHostStore(secureStore);

    await hosts.save(pairedHost);
    expect(await hosts.list()).toEqual([pairedHost]);

    await hosts.remove(pairedHost.id);
    expect(await hosts.list()).toEqual([]);
    expect(secureStore.entries).toEqual([]);
  });

  it("rejects corrupt persisted profiles", async () => {
    const secureStore = new FakeSecureStore();
    await secureStore.setItemAsync(PAIRED_HOSTS_SECURE_STORE_KEY, JSON.stringify([{ nope: true }]));
    const hosts = new PairedHostStore(secureStore);

    await expect(hosts.list()).rejects.toThrow("invalid paired host store");
  });
});
