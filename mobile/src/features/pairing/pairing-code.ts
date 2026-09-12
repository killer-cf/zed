import { decodePairingUrl, type PairingOffer } from "../../protocol";

export function decodePairingInput(value: string): PairingOffer {
  return decodePairingUrl(value.trim());
}

export function parsePairingCode(value: string): PairingOffer {
  return decodePairingInput(value);
}

export function parsePairingLink(value: string): PairingOffer {
  return decodePairingInput(value);
}

export type { PairingOffer } from "../../protocol";
