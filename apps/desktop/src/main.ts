// Desktop app entry point
// This app uses the web app as its frontend, started via Tauri
// The server is started from the Rust side

import { invoke } from '@tauri-apps/api/core';

// TypeScript entry point for the desktop app
export function getAppName(): string {
    return "Git Agent Harness Desktop";
}

/**
 * Invokes the Tauri command to check for a custom aggregator URL.
 * Accepts an environment variable GAH_AGGREGATOR_URL from the host system.
 */
export async function getAggregatorUrl(): Promise<string | null> {
    try {
        return await invoke<string | null>('get_aggregator_url');
    } catch (e) {
        console.error('Failed to get aggregator URL:', e);
        return null;
    }
}