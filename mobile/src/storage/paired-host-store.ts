import { z } from "zod";

export type PairedHost = {
  id: string;
  endpoint: string;
  hostPublicKey: string;
  grantId: string;
  token: string;
  deviceSecretKey: string;
  displayName: string;
};

export type SecureStoreLike = {
  getItemAsync(key: string): Promise<string | null>;
  setItemAsync(key: string, value: string): Promise<void>;
  deleteItemAsync(key: string): Promise<void>;
};
export type PairedHostStoreLike = {
  list(): Promise<PairedHost[]>;
  save(host: PairedHost): Promise<void>;
  remove(id: string): Promise<void>;
};

export const PAIRED_HOSTS_SECURE_STORE_KEY = "zed-mobile-paired-hosts-v1";

const pairedHostSchema = z
  .object({
    id: z.string().min(1),
    endpoint: z.string().min(1),
    hostPublicKey: z.string().min(1),
    grantId: z.string().min(1),
    token: z.string().min(1),
    deviceSecretKey: z.string().min(1),
    displayName: z.string().min(1),
  })
  .strict();

const pairedHostsSchema = z.array(pairedHostSchema).superRefine((hosts, context) => {
  const ids = new Set<string>();
  for (const [index, host] of hosts.entries()) {
    if (ids.has(host.id)) {
      context.addIssue({
        code: z.ZodIssueCode.custom,
        path: [index, "id"],
        message: "duplicate host id",
      });
    }
    ids.add(host.id);
  }
});

const defaultSecureStore: SecureStoreLike = {
  async getItemAsync(key) {
    const secureStore = await import("expo-secure-store");
    return secureStore.getItemAsync(key);
  },
  async setItemAsync(key, value) {
    const secureStore = await import("expo-secure-store");
    await secureStore.setItemAsync(key, value);
  },
  async deleteItemAsync(key) {
    const secureStore = await import("expo-secure-store");
    await secureStore.deleteItemAsync(key);
  },
};

export class PairedHostStore {
  private readonly secureStore: SecureStoreLike;
  private readonly key: string;
  private mutation: Promise<void> = Promise.resolve();

  constructor(
    secureStore: SecureStoreLike = defaultSecureStore,
    key: string = PAIRED_HOSTS_SECURE_STORE_KEY,
  ) {
    this.secureStore = secureStore;
    this.key = key;
  }

  async list(): Promise<PairedHost[]> {
    await this.mutation;
    return this.read();
  }

  async save(host: PairedHost): Promise<void> {
    pairedHostSchema.parse(host);
    return this.mutate(async () => {
      const hosts = await this.read();
      const nextHosts = hosts.filter((existing) => existing.id !== host.id);
      nextHosts.push({ ...host });
      await this.secureStore.setItemAsync(this.key, JSON.stringify(nextHosts));
    });
  }

  async remove(id: string): Promise<void> {
    if (id.length === 0) return;
    return this.mutate(async () => {
      const hosts = await this.read();
      const nextHosts = hosts.filter((host) => host.id !== id);
      if (nextHosts.length === hosts.length) return;
      if (nextHosts.length === 0) {
        await this.secureStore.deleteItemAsync(this.key);
      } else {
        await this.secureStore.setItemAsync(this.key, JSON.stringify(nextHosts));
      }
    });
  }

  private async read(): Promise<PairedHost[]> {
    const encoded = await this.secureStore.getItemAsync(this.key);
    if (encoded === null) return [];

    try {
      const parsed: unknown = JSON.parse(encoded);
      return pairedHostsSchema.parse(parsed).map((host) => ({ ...host }));
    } catch {
      throw new Error("invalid paired host store");
    }
  }

  private async mutate(operation: () => Promise<void>): Promise<void> {
    const next = this.mutation.then(operation);
    this.mutation = next.catch(() => undefined);
    return next;
  }
}

export function createPairedHostStore(
  secureStore: SecureStoreLike = defaultSecureStore,
  key: string = PAIRED_HOSTS_SECURE_STORE_KEY,
): PairedHostStore {
  return new PairedHostStore(secureStore, key);
}

export const pairedHostStore = new PairedHostStore();
