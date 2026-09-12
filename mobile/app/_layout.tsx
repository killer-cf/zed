// React 19 is intentionally runtime-only in the pinned mobile package.
// @ts-expect-error React does not ship declarations in this package.
import * as React from "react";
import * as Linking from "expo-linking";

import { Stack, useRouter } from "expo-router";

const { useEffect } = React;
export default function RootLayout() {
  const router = useRouter();

  useEffect(() => {
    let active = true;
    const openPairingLink = (url: string | null) => {
      if (!active || url === null || !url.startsWith("zed-mobile://pair?")) return;
      router.replace({ pathname: "/pair", params: { code: url } });
    };

    void Linking.getInitialURL().then(openPairingLink);
    const subscription = Linking.addEventListener("url", (event: { url: string }) =>
      openPairingLink(event.url),
    );
    return () => {
      active = false;
      subscription.remove();
    };
  }, [router]);

  return <Stack screenOptions={{ headerShown: true }} />;
}
