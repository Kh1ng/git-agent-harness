// Desktop app entry point
// This app uses the web app as its frontend, started via Tauri
// The server is started from the Rust side

// TypeScript entry point for the desktop app
// The actual functionality is implemented in Rust via Tauri commands
export function getAppName(): string {
    return "Git Agent Harness Desktop";
}