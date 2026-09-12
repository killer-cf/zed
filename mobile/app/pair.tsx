import * as Camera from "expo-camera";
import type { BarcodeScanningResult } from "expo-camera";
import * as Clipboard from "expo-clipboard";
import * as Linking from "expo-linking";
import { useLocalSearchParams, useRouter } from "expo-router";
// @ts-expect-error React does not ship declarations in this package.
import * as React from "react";
import { Pressable, StyleSheet, Text, TextInput, View } from "react-native";

import { decodePairingInput } from "../src/features/pairing/pairing-code";
import {
  pairedHostStore,
  type PairedHost,
  type PairedHostStoreLike,
} from "../src/storage/paired-host-store";
import { MobileRpcClient } from "../src/transport/mobile-rpc-client";
import type { PairingOffer } from "../src/protocol";

type PairMode = "choice" | "scan" | "paste";
type PairFunction = (offer: PairingOffer, label: string) => Promise<PairedHost>;

export type PairScreenProps = {
  store?: PairedHostStoreLike;
  pair?: PairFunction;
  initialValue?: string;
};

const defaultPair: PairFunction = (offer, label) => MobileRpcClient.pair(offer, label);

export function PairScreen({
  store = pairedHostStore,
  pair = defaultPair,
  initialValue,
}: PairScreenProps) {
  const { useCallback, useEffect, useRef, useState } = React;
  const router = useRouter();
  const params = useLocalSearchParams<{ code?: string }>();
  const [mode, setMode] = useState<PairMode>("choice");
  const [cameraGranted, setCameraGranted] = useState(false);
  const [permissionDenied, setPermissionDenied] = useState(false);
  const [value, setValue] = useState(initialValue ?? "");
  const [label, setLabel] = useState("This phone");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const pendingProfile = useRef<PairedHost>();
  const [canRetryPersistence, setCanRetryPersistence] = useState(false);
  const pairingInProgress = useRef(false);

  const persistProfile = useCallback(
    async (profile: PairedHost): Promise<boolean> => {
      try {
        await store.save(profile);
        pendingProfile.current = undefined;
        setCanRetryPersistence(false);
        router.replace("/");
        return true;
      } catch {
        pendingProfile.current = profile;
        setCanRetryPersistence(true);
        setError("Pairing completed, but the host profile could not be saved. Retry without scanning again.");
        return false;
      }
    },
    [router, store],
  );

  const consume = useCallback(
    async (rawValue: string) => {
      if (pairingInProgress.current || pendingProfile.current) return;
      pairingInProgress.current = true;
      setBusy(true);
      setError(undefined);
      try {
        const offer = decodePairingInput(toPairingUrl(rawValue));
        const profile = await pair(offer, label.trim() || "This phone");
        if (!(await persistProfile(profile))) {
          pairingInProgress.current = false;
          setBusy(false);
        }
      } catch (caughtError: unknown) {
        const message = caughtError instanceof Error ? caughtError.message : "Unable to pair host";
        setError(message);
        pairingInProgress.current = false;
        setBusy(false);
      }
    },
    [label, pair, persistProfile],
  );

  const retryPersistence = useCallback(async () => {
    const profile = pendingProfile.current;
    if (pairingInProgress.current || !profile) return;
    pairingInProgress.current = true;
    setBusy(true);
    setError(undefined);
    if (!(await persistProfile(profile))) {
      pairingInProgress.current = false;
      setBusy(false);
    }
  }, [persistProfile]);

  useEffect(() => {
    const routeValue = initialValue ?? params.code;
    if (typeof routeValue === "string" && routeValue.length > 0) void consume(routeValue);
  }, [consume, initialValue, params.code]);

  useEffect(() => {
    const subscription = Linking.addEventListener("url", ({ url }) => {
      void consume(url);
    });
    return () => subscription.remove();
  }, [consume]);

  const chooseScan = async () => {
    if (busy) return;
    setError(undefined);
    setMode("scan");
    const permission = await Camera.Camera.requestCameraPermissionsAsync();
    if (permission.granted) {
      setCameraGranted(true);
      setPermissionDenied(false);
    } else {
      setCameraGranted(false);
      setPermissionDenied(true);
      setMode("paste");
    }
  };

  const choosePaste = () => {
    if (busy) return;
    setError(undefined);
    setMode("paste");
  };

  const pasteFromClipboard = async () => {
    if (busy) return;
    try {
      const clipboardValue = await Clipboard.getStringAsync();
      setValue(clipboardValue);
      await consume(clipboardValue);
    } catch (caughtError: unknown) {
      setError(caughtError instanceof Error ? caughtError.message : "Unable to read clipboard");
    }
  };

  const onBarcodeScanned = (result: BarcodeScanningResult) => {
    void consume(result.data);
  };

  return (
    <View style={styles.container}>
      <Text style={styles.title}>Pair a Zed host</Text>
      <Text style={styles.body}>Use a pairing QR code or paste its zed-mobile link.</Text>
      {canRetryPersistence ? (
        <View style={styles.pasteForm}>
          <Text style={styles.body}>The host profile is ready to save.</Text>
          <Pressable accessibilityRole="button" disabled={busy} onPress={() => void retryPersistence()}>
            <Text style={styles.action}>Retry saving host</Text>
          </Pressable>
        </View>
      ) : (
        <>
          {mode === "choice" ? (
            <View style={styles.choices}>
              <Pressable accessibilityRole="button" disabled={busy} onPress={() => void chooseScan()}>
                <Text style={styles.action}>Scan QR code</Text>
              </Pressable>
              <Pressable accessibilityRole="button" disabled={busy} onPress={choosePaste}>
                <Text style={styles.action}>Paste pairing link</Text>
              </Pressable>
            </View>
          ) : null}
          {mode === "scan" && cameraGranted ? (
            <View style={styles.scanner}>
              <Camera.CameraView
                accessibilityLabel="Pairing QR scanner"
                barcodeScannerSettings={{ barcodeTypes: ["qr"] }}
                onBarcodeScanned={busy ? undefined : onBarcodeScanned}
                style={styles.camera}
              />
              <Text style={styles.body}>Point the camera at the QR code shown by Zed Desktop.</Text>
            </View>
          ) : null}
          {mode === "paste" || permissionDenied ? (
            <View style={styles.pasteForm}>
              <Text style={styles.body}>Paste pairing link</Text>
              {permissionDenied ? (
                <Text style={styles.body}>Camera permission was denied. Paste the pairing link instead.</Text>
              ) : null}
              <TextInput
                autoCapitalize="none"
                autoCorrect={false}
                editable={!busy}
                onChangeText={setValue}
                placeholder="zed-mobile://pair?code=..."
                style={styles.input}
                value={value}
              />
              <Pressable accessibilityRole="button" disabled={busy} onPress={() => void pasteFromClipboard()}>
                <Text style={styles.action}>Paste from clipboard</Text>
              </Pressable>
              <Pressable accessibilityRole="button" disabled={busy} onPress={() => void consume(value)}>
                <Text style={styles.action}>Pair host</Text>
              </Pressable>
            </View>
          ) : null}
        </>
      )}
      <TextInput
        autoCapitalize="words"
        editable={!busy}
        onChangeText={setLabel}
        placeholder="Device label"
        style={styles.input}
        value={label}
      />
      {busy ? <Text style={styles.body}>Pairing securely…</Text> : null}
      {error ? <Text style={styles.error}>{error}</Text> : null}
      <Pressable accessibilityRole="button" disabled={busy} onPress={() => router.back()}>
        <Text style={styles.cancel}>Cancel</Text>
      </Pressable>
    </View>
  );
}

export default function PairRoute() {
  return <PairScreen />;
}

function toPairingUrl(value: string): string {
  const trimmed = value.trim();
  return trimmed.startsWith("zed-mobile://") ? trimmed : `zed-mobile://pair?code=${trimmed}`;
}

const styles = StyleSheet.create({
  container: {
    flex: 1,
    gap: 16,
    padding: 24,
  },
  title: {
    fontSize: 28,
    fontWeight: "700",
  },
  body: {
    color: "#555",
    lineHeight: 22,
  },
  choices: {
    gap: 16,
    marginTop: 16,
  },
  scanner: {
    gap: 12,
  },
  camera: {
    height: 320,
    width: "100%",
  },
  pasteForm: {
    gap: 12,
  },
  input: {
    borderColor: "#bbb",
    borderRadius: 8,
    borderWidth: 1,
    padding: 12,
  },
  action: {
    color: "#1769aa",
    fontWeight: "600",
  },
  cancel: {
    color: "#555",
  },
  error: {
    color: "#b3261e",
  },
});
