/** Device pairing grants existing dashboard control, not a sandboxed capability role. */
export interface PairedDevice {
  id: string;
  name: string;
  created_at: string;
  expires_at: string;
  revoked_at: string | null;
  /** Only the owner may enable this. Omitted on legacy devices means false. */
  can_approve_pairing?: boolean;
}
export interface PairingOffer {
  schema_version: 1;
  code: string;
  expires_at: string;
  server: { id: string; name: string; origin: string };
  access: string;
}
export type PairingPreview = Omit<PairingOffer, 'code'>;

/** The matching code is for human comparison; it cannot claim device access. */
export interface PairingAccessRequest {
  schema_version: 1;
  id: string;
  name: string;
  matching_code: string;
  expires_at: string;
  server: PairingOffer['server'];
  access: string;
  status: 'pending' | 'approved' | 'denied' | 'expired' | 'claimed';
}
