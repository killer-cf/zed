import { z } from "zod";

export const protocolVersion = 1 as const;
export const minimumCompatibleDesktopVersion = 1 as const;

const pairingUrlPrefix = "zed-mobile://pair?code=";
const pairingDomain = new TextEncoder().encode("zed-mobile/pair/v1");
const authenticationDomain = new TextEncoder().encode("zed-mobile/auth/v1");

const uuidSchema = z.string().uuid();
const u16Schema = z.number().int().min(0).max(0xffff);
const isoTimestampSchema = z.string().datetime({ offset: true });

const capabilityNames = [
  "status_read",
  "threads_read",
  "threads_control",
  "terminal_stream",
  "terminal_input",
  "chat_send",
  "attachments_send",
  "worktrees_read",
  "worktrees_create",
  "files_read",
  "source_control_write",
  "quick_commands",
  "accounts_read",
  "accounts_switch",
  "usage_read",
  "notifications",
  "browser_mobile_view",
] as const;

export const capabilitySchema = z.enum(capabilityNames);
export type Capability = z.infer<typeof capabilitySchema>;

function sortCapabilities(capabilities: readonly Capability[]): Capability[] {
  return [...capabilities].sort(
    (left, right) => capabilityNames.indexOf(left) - capabilityNames.indexOf(right),
  );
}

const pairingOfferWireSchema = z
  .object({
    offer_id: uuidSchema,
    endpoint: z.string(),
    protocol_version: u16Schema,
    host_public_key: z.string(),
    pairing_secret: z.string(),
    expires_at: isoTimestampSchema,
  })
  .strict();
export type PairingOfferWire = z.infer<typeof pairingOfferWireSchema>;

export const pairingOfferSchema = pairingOfferWireSchema.transform((offer) => ({
  offerId: offer.offer_id,
  endpoint: offer.endpoint,
  protocolVersion: offer.protocol_version,
  hostPublicKey: offer.host_public_key,
  pairingSecret: offer.pairing_secret,
  expiresAt: offer.expires_at,
}));

export type PairingOffer = z.infer<typeof pairingOfferSchema>;

export function parsePairingOffer(input: unknown): PairingOffer {
  return pairingOfferSchema.parse(input);
}

const statusWireSchema = z
  .object({
    host_name: z.string(),
    protocol_version: u16Schema,
    minimum_compatible_mobile_version: u16Schema,
    capabilities: z.array(capabilitySchema),
  })
  .strict()
  .transform((status) => ({
    host_name: status.host_name,
    protocol_version: status.protocol_version,
    minimum_compatible_mobile_version: status.minimum_compatible_mobile_version,
    capabilities: sortCapabilities(status.capabilities),
  }));

export const statusSchema = statusWireSchema;
export type Status = z.infer<typeof statusSchema>;

export function parseStatus(input: unknown): Status {
  return statusSchema.parse(input);
}

const optionalUuidSchema = z.union([uuidSchema, z.null()]).default(null);

const clientFrameVariants = [
  z
    .object({
      type: z.literal("pair_begin"),
      offer_id: uuidSchema,
      client_nonce: z.string(),
    })
    .strict(),
  z
    .object({
      type: z.literal("pair_complete"),
      offer_id: uuidSchema,
      pairing_secret: z.string(),
      client_public_key: z.string(),
      client_proof: z.string(),
      device_label: z.string(),
    })
    .strict(),
  z
    .object({
      type: z.literal("connect"),
      grant_id: uuidSchema,
      client_nonce: z.string(),
    })
    .strict(),
  z
    .object({
      type: z.literal("authenticate"),
      grant_id: uuidSchema,
      token: z.string(),
      client_proof: z.string(),
    })
    .strict(),
  z
    .object({
      type: z.literal("request"),
      request_id: uuidSchema,
      operation_id: optionalUuidSchema,
      method: z.string(),
      params: z.unknown(),
    })
    .strict(),
  z
    .object({
      type: z.literal("ping"),
      nonce: z.string(),
    })
    .strict(),
] as const;

export const clientFrameSchema = z.discriminatedUnion("type", clientFrameVariants);
export type ClientFrame = z.infer<typeof clientFrameSchema>;

export function parseClientFrame(input: unknown): ClientFrame {
  const result = clientFrameSchema.safeParse(input);
  if (!result.success) {
    throw new Error("invalid client frame");
  }
  return result.data;
}

const serverFrameVariants = [
  z
    .object({
      type: z.literal("pair_challenge"),
      offer_id: uuidSchema,
      client_nonce: z.string(),
      server_nonce: z.string(),
      host_signature: z.string(),
    })
    .strict(),
  z
    .object({
      type: z.literal("pair_complete"),
      grant_id: uuidSchema,
      token: z.string(),
    })
    .strict(),
  z
    .object({
      type: z.literal("server_challenge"),
      grant_id: uuidSchema,
      client_nonce: z.string(),
      server_nonce: z.string(),
      host_signature: z.string(),
    })
    .strict(),
  z
    .object({
      type: z.literal("authenticated"),
      grant_id: uuidSchema,
    })
    .strict(),
  z
    .object({
      type: z.literal("response"),
      request_id: uuidSchema,
      operation_id: optionalUuidSchema,
      result: z.unknown(),
    })
    .strict(),
  z
    .object({
      type: z.literal("event"),
      event: z.string(),
      payload: z.unknown(),
    })
    .strict(),
  z
    .object({
      type: z.literal("pong"),
      nonce: z.string(),
    })
    .strict(),
  z
    .object({
      type: z.literal("error"),
      request_id: optionalUuidSchema,
      code: z.string(),
      message: z.string(),
    })
    .strict(),
] as const;

export const serverFrameSchema = z.discriminatedUnion("type", serverFrameVariants);
export type ServerFrame = z.infer<typeof serverFrameSchema>;

export function parseServerFrame(input: unknown): ServerFrame {
  const result = serverFrameSchema.safeParse(input);
  if (!result.success) {
    throw new Error("invalid server frame");
  }
  return result.data;
}

export function decodePairingUrl(url: string): PairingOffer {
  if (!url.startsWith(pairingUrlPrefix)) {
    throw new Error("invalid pairing URL");
  }

  const encodedOffer = url.slice(pairingUrlPrefix.length);
  if (encodedOffer.length === 0 || encodedOffer.includes("&")) {
    throw new Error("invalid pairing URL");
  }

  try {
    const decodedOffer = decodeBase64Url(encodedOffer);
    const encodedJson = new TextDecoder("utf-8", { fatal: true }).decode(decodedOffer);
    return parsePairingOffer(JSON.parse(encodedJson) as unknown);
  } catch {
    throw new Error("invalid pairing URL");
  }
}

export function encodePairingUrl(offer: PairingOffer): string {
  const wireOffer: PairingOfferWire = {
    offer_id: offer.offerId,
    endpoint: offer.endpoint,
    protocol_version: offer.protocolVersion,
    host_public_key: offer.hostPublicKey,
    pairing_secret: offer.pairingSecret,
    expires_at: offer.expiresAt,
  };
  const encodedJson = new TextEncoder().encode(JSON.stringify(wireOffer));
  return `${pairingUrlPrefix}${encodeBase64Url(encodedJson)}`;
}

export function pairingPayload(
  version: number,
  offerId: string,
  clientNonce: string,
  serverNonce: string,
): Uint8Array {
  return buildSigningPayload(pairingDomain, version, offerId, [
    decodeBase64Url(clientNonce),
    decodeBase64Url(serverNonce),
  ]);
}

export function authenticationPayload(
  version: number,
  grantId: string,
  clientNonce: string,
  serverNonce: string,
  tokenDigest: Uint8Array,
): Uint8Array {
  if (tokenDigest.length !== 32) {
    throw new Error("authentication token digest must be 32 bytes");
  }
  return buildSigningPayload(authenticationDomain, version, grantId, [
    decodeBase64Url(clientNonce),
    decodeBase64Url(serverNonce),
    tokenDigest,
  ]);
}

function buildSigningPayload(
  domain: Uint8Array,
  version: number,
  identifier: string,
  fields: readonly Uint8Array[],
): Uint8Array {
  if (!Number.isInteger(version) || version < 0 || version > 0xffff) {
    throw new Error("protocol version must fit in an unsigned 16-bit integer");
  }

  const identifierBytes = parseUuidBytes(identifier);
  const payloadLength =
    domain.length +
    2 +
    identifierBytes.length +
    fields.reduce((length, field) => length + 4 + field.length, 0);
  const payload = new Uint8Array(payloadLength);
  const view = new DataView(payload.buffer, payload.byteOffset, payload.byteLength);
  let offset = 0;

  payload.set(domain, offset);
  offset += domain.length;
  view.setUint16(offset, version, false);
  offset += 2;
  payload.set(identifierBytes, offset);
  offset += identifierBytes.length;

  for (const field of fields) {
    view.setUint32(offset, field.length, false);
    offset += 4;
    payload.set(field, offset);
    offset += field.length;
  }

  return payload;
}

function parseUuidBytes(identifier: string): Uint8Array {
  if (!uuidSchema.safeParse(identifier).success) {
    throw new Error("invalid UUID");
  }

  const compactIdentifier = identifier.replaceAll("-", "");
  const bytes = new Uint8Array(16);
  for (let index = 0; index < bytes.length; index += 1) {
    bytes[index] = Number.parseInt(compactIdentifier.slice(index * 2, index * 2 + 2), 16);
  }
  return bytes;
}

function decodeBase64Url(value: string): Uint8Array {
  if (!/^[A-Za-z0-9_-]*={0,2}$/.test(value)) {
    throw new Error("invalid base64url data");
  }

  const unpaddedValue = value.replace(/=+$/, "");
  const remainder = unpaddedValue.length % 4;
  if (remainder === 1) {
    throw new Error("invalid base64url data");
  }

  const requiredPadding = (4 - remainder) % 4;
  const suppliedPadding = value.length - unpaddedValue.length;
  if (suppliedPadding !== 0 && suppliedPadding !== requiredPadding) {
    throw new Error("invalid base64url data");
  }

  const paddedValue = `${unpaddedValue}${"=".repeat(requiredPadding)}`;
  const standardBase64 = paddedValue.replaceAll("-", "+").replaceAll("_", "/");
  const binary = globalThis.atob(standardBase64);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return bytes;
}

function encodeBase64Url(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  return globalThis.btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/, "");
}
