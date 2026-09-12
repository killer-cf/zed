# Zed Mobile

`zed-mobile` is a standalone Expo companion package for the Zed Desktop mobile protocol. It is intentionally independent of the Cargo workspace. Task 5 provides the protocol mirror and package foundation; app routes and host operations are added by later tasks rather than represented by fake feature screens.

## Prerequisites

- Node.js 24.
- pnpm 9 (the package pins `pnpm@9.15.9`).
- Xcode for iOS development or Android Studio for Android development.
- An Expo development client built for the native dependencies in this package.
- Tailscale installed and connected on both the Zed host and the phone. The host endpoint is reachable through the same tailnet.

Expo Go cannot load native development-client dependencies used by this package. Use a development-client build instead of opening this project in Expo Go.

## Focused commands

From the repository root:

```bash
pnpm --dir mobile test
pnpm --dir mobile typecheck
pnpm --dir mobile lint
```

The same scripts work from the package directory:

```bash
cd mobile
pnpm test
pnpm typecheck
pnpm lint
```

To launch the native development client, use `pnpm --dir mobile start` after installing the platform prerequisites. The only registered deep-link scheme is `zed-mobile://pair?code=...`.

## Protocol boundary

`src/protocol.ts` mirrors the version-one Rust DTOs, strict JSON envelopes, pairing URL codec, capability names, and signing-payload byte vectors. Pairing inputs from scanning, paste, or a deep link must use the same `decodePairingUrl` function when those routes are added.
