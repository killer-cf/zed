import { useIsFocused, useRouter } from "expo-router";
// @ts-expect-error React does not ship declarations in this package.
import * as React from "react";
import { Pressable, ScrollView, StyleSheet, Text, View } from "react-native";
import {
  MobileRpcClient,
  type ConnectionState,
} from "../src/transport/mobile-rpc-client";
import {
  pairedHostStore,
  type PairedHost,
  type PairedHostStoreLike,
} from "../src/storage/paired-host-store";
import type { Status } from "../src/protocol";

type HostClient = Pick<MobileRpcClient, "connect" | "retry" | "close" | "status">;
type HostClientFactory = (store: PairedHostStoreLike) => HostClient;

type HostEntry = {
  host: PairedHost;
  state: ConnectionState;
  status?: Status;
  error?: string;
};

export type HostDashboardProps = {
  store?: PairedHostStoreLike;
  createClient?: HostClientFactory;
};

const defaultClientFactory: HostClientFactory = (store) => new MobileRpcClient({ hostStore: store });

export function HostDashboard({
  store = pairedHostStore,
  createClient = defaultClientFactory,
}: HostDashboardProps) {
  const router = useRouter();
  const isFocused = useIsFocused();
  const { useCallback, useEffect, useRef, useState } = React;
  const [entries, setEntries] = useState<HostEntry[]>([]);
  const [loadError, setLoadError] = useState<string>();
  const clients = useRef(new Map<string, HostClient>());

  const updateEntry = useCallback((hostId: string, update: Partial<HostEntry>) => {
    setEntries((current: HostEntry[]) =>
      current.map((entry: HostEntry) => (entry.host.id === hostId ? { ...entry, ...update } : entry)),
    );
  }, []);

  const connectHost = useCallback(
    (host: PairedHost) => {
      let client = clients.current.get(host.id);
      if (!client) {
        client = createClient(store);
        clients.current.set(host.id, client);
      }
      const activeClient = client;
      updateEntry(host.id, { state: activeClient.status(), error: undefined });
      void activeClient
        .connect(host)
        .then((status: Status) => {
          if (clients.current.get(host.id) !== activeClient) return;
          updateEntry(host.id, { state: activeClient.status(), status, error: undefined });
        })
        .catch((error: unknown) => {
          if (clients.current.get(host.id) !== activeClient) return;
          const message = error instanceof Error ? error.message : "Unable to connect";
          updateEntry(host.id, { state: activeClient.status() ?? "unreachable", error: message });
        });
    },
    [createClient, store, updateEntry],
  );

  useEffect(() => {
    if (!isFocused) return;
    let active = true;
    void store
      .list()
      .then((hosts) => {
        if (!active) return;
        const hostIds = new Set(hosts.map((host) => host.id));
        for (const [hostId, client] of clients.current) {
          if (!hostIds.has(hostId)) {
            client.close();
            clients.current.delete(hostId);
          }
        }
        setLoadError(undefined);
        setEntries(hosts.map((host) => ({ host, state: "disconnected" })));
        for (const host of hosts) connectHost(host);
      })
      .catch((error: unknown) => {
        if (!active) return;
        const message = error instanceof Error ? error.message : "Unable to load paired hosts";
        setLoadError(message);
      });

    return () => {
      active = false;
      for (const client of clients.current.values()) client.close();
      clients.current.clear();
    };
  }, [connectHost, isFocused, store]);

  useEffect(() => {
    const timer = setInterval(() => {
      setEntries((current: HostEntry[]) =>
        current.map((entry: HostEntry) => {
          const client = clients.current.get(entry.host.id);
          if (!client) return entry;
          const state = client.status();
          return {
            ...entry,
            state,
            error: state === "connected" || state === "reconnecting" ? undefined : entry.error,
          };
        }),
      );
    }, 500);
    return () => clearInterval(timer);
  }, []);

  const retryHost = (entry: HostEntry) => {
    const client = clients.current.get(entry.host.id);
    if (!client) {
      connectHost(entry.host);
      return;
    }
    client.retry();
    updateEntry(entry.host.id, { state: client.status(), error: undefined });
  };

  const removeHost = async (entry: HostEntry) => {
    clients.current.get(entry.host.id)?.close();
    await store.remove(entry.host.id);
    clients.current.delete(entry.host.id);
    setEntries((current: HostEntry[]) =>
      current.filter((candidate: HostEntry) => candidate.host.id !== entry.host.id),
    );
  };

  return (
    <ScrollView contentContainerStyle={styles.container}>
      <View style={styles.header}>
        <Text style={styles.title}>Paired hosts</Text>
        <Pressable accessibilityRole="button" onPress={() => router.push("/pair")}>
          <Text style={styles.action}>Pair host</Text>
        </Pressable>
      </View>
      {entries.length === 0 ? (
        <View style={styles.emptyState}>
          <Text style={styles.emptyTitle}>{loadError ?? "No paired hosts"}</Text>
          <Text style={styles.emptyBody}>
            {loadError ? "Secure host profiles could not be loaded." : "Pair a Zed Desktop host to see its live status here."}
          </Text>
        </View>
      ) : (
        entries.map((entry: HostEntry) => (
          // @ts-expect-error React Native's incomplete local typings omit JSX's reserved key prop.
          <View key={entry.host.id} style={styles.hostCard}>
            <Text style={styles.hostName}>{entry.host.displayName}</Text>
            <Text style={styles.endpoint}>{entry.host.endpoint}</Text>
            <Text accessibilityLabel={`Connection state: ${entry.state}`} style={styles.state}>
              {entry.state}
            </Text>
            {entry.state === "incompatible" ? (
              <Text style={styles.reason}>This host uses an incompatible mobile protocol version.</Text>
            ) : null}
            {entry.error && entry.state !== "incompatible" ? (
              <Text style={styles.reason}>{entry.error}</Text>
            ) : null}
            <View style={styles.actions}>
              <Pressable accessibilityRole="button" onPress={() => retryHost(entry)}>
                <Text style={styles.action}>Retry</Text>
              </Pressable>
              <Pressable accessibilityRole="button" onPress={() => void removeHost(entry)}>
                <Text style={styles.removeAction}>Remove</Text>
              </Pressable>
            </View>
          </View>
        ))
      )}
    </ScrollView>
  );
}

export default function IndexScreen() {
  return <HostDashboard />;
}


const styles = StyleSheet.create({
  container: {
    flexGrow: 1,
    padding: 24,
    gap: 16,
  },
  header: {
    alignItems: "center",
    flexDirection: "row",
    justifyContent: "space-between",
  },
  title: {
    fontSize: 28,
    fontWeight: "700",
  },
  action: {
    color: "#1769aa",
    fontWeight: "600",
  },
  removeAction: {
    color: "#b3261e",
    fontWeight: "600",
  },
  emptyState: {
    alignItems: "center",
    gap: 8,
    paddingVertical: 64,
  },
  emptyTitle: {
    fontSize: 20,
    fontWeight: "600",
  },
  emptyBody: {
    color: "#666",
    textAlign: "center",
  },
  hostCard: {
    borderColor: "#ddd",
    borderRadius: 12,
    borderWidth: 1,
    gap: 8,
    padding: 16,
  },
  hostName: {
    fontSize: 20,
    fontWeight: "600",
  },
  endpoint: {
    color: "#555",
    fontFamily: "monospace",
  },
  state: {
    fontWeight: "600",
    textTransform: "capitalize",
  },
  reason: {
    color: "#8a1c1c",
  },
  actions: {
    flexDirection: "row",
    gap: 20,
    marginTop: 8,
  },
});
