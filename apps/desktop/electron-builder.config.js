/**
 * Electron Builder Configuration
 * https://www.electron.build/configuration
 */

import { defineConfig } from 'electron-builder';

export default defineConfig({
  appId: 'com.git-agent-harness.desktop',
  productName: 'Git Agent Harness Desktop',
  artifactName: '${productName}-${version}-${arch}.${ext}',
  
  directories: {
    output: 'dist-electron-packages',
    app: 'dist',
  },
  
  files: [
    '**/*',
    'dist/**/*',
    'dist-electron/**/*',
  ],
  
  extraResources: [
    {
      from: 'apps/server/dist',
      to: 'server',
      filter: ['**/*'],
    },
  ],
  
  // Platform specific builds
  win: {
    target: ['nsis'],
    icon: 'assets/icon.ico',
  },
  
  mac: {
    target: ['dmg'],
    icon: 'assets/icon.icns',
    entitlements: 'entitlements.mac.plist',
    entitlementsInherit: 'entitlements.mac.plist',
    hardenedRuntime: true,
    gatekeeperAssess: false,
  },
  
  linux: {
    target: ['AppImage'],
    icon: 'assets/icon.png',
    category: 'Development',
    executableName: 'git-agent-harness-desktop',
  },
  
  // NSIS specific options
  nsis: {
    oneClick: false,
    allowToChangeInstallationDirectory: true,
    installerIcon: 'assets/icon.ico',
    uninstallerIcon: 'assets/icon.ico',
    installerHeaderIcon: 'assets/icon.ico',
    createDesktopShortcut: true,
    createStartMenuShortcut: true,
  },
  
  // DMG specific options
  dmg: {
    icon: 'assets/icon.icns',
    iconSize: 128,
    window: {
      width: 540,
      height: 380,
    },
    contents: [
      { x: 142, y: 158 },
      { x: 398, y: 158, type: 'link', path: '/Applications' },
    ],
  },
  
  // Metadata
  asar: true,
  asarUnpack: ['**/node_modules/**'],
  
  compression: 'store',
  
  publish: null, // Disable auto-publishing
});