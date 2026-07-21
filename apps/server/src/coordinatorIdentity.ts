import { readFileSync, writeFileSync, existsSync, mkdirSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import crypto from 'node:crypto';

export interface CoordinatorIdentity {
  node_id: string;
  display_name: string;
  advertised_url: string;
  version: string;
  schema_digest: string;
}

export const COORDINATOR_SCHEMA_DIGEST = crypto
  .createHash('sha256')
  .update('gah-coordinator-v1')
  .digest('hex');

let cachedIdentity: { cacheKey: string; identity: CoordinatorIdentity } | null = null;

export function getCoordinatorIdentity(
  configPath?: string,
  port: number = 3773
): CoordinatorIdentity {
  const identityPath = configPath || process.env.GAH_COORDINATOR_IDENTITY_PATH || resolve(process.cwd(), 'config/coordinator-identity.json');
  const cacheKey = `${identityPath}::${port}`;
  if (cachedIdentity?.cacheKey === cacheKey) return cachedIdentity.identity;
  
  let node_id: string;
  let display_name = 'GAH Coordinator';
  let advertised_url = `http://localhost:${port}`;
  const version = '0.1.0';
  const schema_digest = COORDINATOR_SCHEMA_DIGEST;

  if (existsSync(identityPath)) {
    try {
      const data = JSON.parse(readFileSync(identityPath, 'utf8'));
      node_id = data.node_id || crypto.randomUUID();
      if (data.display_name) display_name = data.display_name;
      if (data.advertised_url) advertised_url = data.advertised_url;
    } catch {
      node_id = crypto.randomUUID();
    }
  } else {
    node_id = crypto.randomUUID();
    try {
      const dir = dirname(identityPath);
      if (!existsSync(dir)) {
        mkdirSync(dir, { recursive: true });
      }
      writeFileSync(identityPath, JSON.stringify({ node_id, display_name, advertised_url }, null, 2));
    } catch (e) {
      // ignore
    }
  }

  const identity = {
    node_id,
    display_name,
    advertised_url,
    version,
    schema_digest
  };
  cachedIdentity = { cacheKey, identity };
  return identity;
}

export function resetCachedCoordinatorIdentity() {
  cachedIdentity = null;
}
