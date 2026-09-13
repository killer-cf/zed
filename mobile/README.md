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

## CI checks

The generated `run_tests` workflow runs the mobile checks independently when
`mobile/**`, the workflow generator, or the generated mobile CI configuration
changes. It installs Node 24 and pnpm 9, caches `mobile/pnpm-lock.yaml`, and
runs the same frozen install, test, typecheck, and lint commands listed above.

The package declares `pnpm@9.15.9`. When pnpm is not installed globally, use
the pinned package through npm:

```bash
npm exec --yes --package=pnpm@9.15.9 -- pnpm --dir mobile test
npm exec --yes --package=pnpm@9.15.9 -- pnpm --dir mobile typecheck
npm exec --yes --package=pnpm@9.15.9 -- pnpm --dir mobile lint
```

## Real-device Tailscale acceptance

Run this sequence only on a non-production development host and a phone that
are authenticated to the same tailnet, with a development-client build of
Zed Mobile. Record only pass/fail/not-run results in the pull request; never
record QR contents, pairing URLs, tokens, keys, prompts, or terminal output.

1. Enable Mobile in Zed, select the host's Tailscale address (`100.x.x.x` or
   the `fd7a:115c:a1e0::/48` range), and keep port `6769`.
2. Generate an offer, scan it in the development build, and confirm one host
   appears as `connected`, with protocol version 1 and only the status
   capability.
3. Background the phone, disable and re-enable its Tailscale route, then
   foreground the app. Confirm the dashboard reconnects or visibly reaches
   `unreachable` with a working `Retry` action.
4. Revoke the phone from Zed's Mobile control window. Confirm the phone becomes
   `revoked`, its secure profile is removed, and it cannot reconnect.
5. Generate a second offer and pair again. Close and reopen Zed without
   changing the binding or port, then confirm the app reconnects to the stable
   endpoint.

If the host, phone, or tailnet is unavailable, mark the acceptance not run
instead of substituting a simulated result.

To launch the native development client, use `pnpm --dir mobile start` after installing the platform prerequisites. The only registered deep-link scheme is `zed-mobile://pair?code=...`.

## Protocol boundary

`src/protocol.ts` mirrors the version-one Rust DTOs, strict JSON envelopes, pairing URL codec, capability names, and signing-payload byte vectors. Pairing inputs from scanning, paste, or a deep link must use the same `decodePairingUrl` function when those routes are added.
