/** Device pairing grants existing dashboard control, not a sandboxed capability role. */
export interface PairedDevice {
  id: string;
  name: string;
  created_at: string;
  expires_at: string;
  revoked_at: string | null;
}
export interface PairingOffer {
  schema_version: 1;
  code: string;
  expires_at: string;
  server: { id: string; name: string; origin: string };
  access: string;
}
export type PairingPreview = Omit<PairingOffer, 'code'>;
