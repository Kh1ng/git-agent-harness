import { app, BrowserWindow, ipcMain } from 'electron';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawn } from 'node:child_process';

const __dirname = fileURLToPath(new URL('.', import.meta.url));

let mainWindow: BrowserWindow | null = null;
let serverProcess: ReturnType<typeof spawn> | null = null;

function createWindow() {
  mainWindow = new BrowserWindow({
    width: 1200,
    height: 800,
    minWidth: 800,
    minHeight: 600,
    webPreferences: {
      nodeIntegration: false,
      contextIsolation: true,
      sandbox: true,
      preload: join(__dirname, 'preload.cjs')
    },
    title: 'Git Agent Harness Desktop'
  });

  // Check if in development mode - Vite/Electron inject import.meta.env
  const isDev = (import.meta as unknown as { env: { DEV?: boolean } }).env?.DEV || false;
  if (isDev) {
    mainWindow.loadURL('http://localhost:3000');
    mainWindow.webContents.openDevTools();
  } else {
    mainWindow.loadFile(join(__dirname, '../../dist/index.html'));
  }

  mainWindow.on('closed', () => {
    mainWindow = null;
  });
}

function startServer(): Promise<void> {
  return new Promise<void>((resolve, reject) => {
    const serverPath = join(__dirname, '../../../apps/server/dist/bin.js');
    
    console.log(`Starting server from: ${serverPath}`);
    
    serverProcess = spawn('node', [serverPath], {
      stdio: ['ignore', 'pipe', 'pipe'],
      cwd: join(__dirname, '../../../apps/server')
    });

    if (serverProcess) {
      serverProcess.stdout?.on('data', (data) => {
        console.log(`[Server] ${data.toString().trim()}`);
        if (data.toString().includes('listening on port')) {
          resolve();
        }
      });

      serverProcess.stderr?.on('data', (data) => {
        console.error(`[Server Error] ${data.toString().trim()}`);
      });

      serverProcess.on('error', (error) => {
        console.error('Server process error:', error);
        reject(error);
      });

      serverProcess.on('exit', (code) => {
        console.log(`Server process exited with code ${code}`);
        if (code !== 0) {
          reject(new Error(`Server process exited with code ${code}`));
        }
      });
    } else {
      reject(new Error('Failed to spawn server process'));
    }
  });
}

function stopServer() {
  if (serverProcess) {
    serverProcess.kill('SIGTERM');
    serverProcess = null;
  }
}

// Set up IPC for server control
ipcMain.handle('start-server', async () => {
  try {
    await startServer();
    return { success: true };
  } catch (error) {
    return { success: false, error: error instanceof Error ? error.message : String(error) };
  }
});

ipcMain.handle('stop-server', async () => {
  stopServer();
  return { success: true };
});

ipcMain.handle('is-server-running', async () => {
  return { running: !!serverProcess && !serverProcess.killed };
});

// Handle app lifecycle
app.on('ready', async () => {
  try {
    // Start the server
    await startServer();
    
    // Wait a bit for server to start
    await new Promise(resolve => setTimeout(resolve, 2000));
    
    // Create the main window
    createWindow();
    
  } catch (error) {
    console.error('Failed to start server:', error);
    createWindow(); // Still create window even if server fails
  }
});

app.on('window-all-closed', () => {
  if (process.platform !== 'darwin') {
    stopServer();
    app.quit();
  }
});

app.on('activate', () => {
  if (mainWindow === null) {
    createWindow();
  }
});

app.on('will-quit', () => {
  stopServer();
});

// Handle uncaught exceptions
process.on('uncaughtException', (error) => {
  console.error('Uncaught Exception:', error);
});

process.on('unhandledRejection', (reason) => {
  console.error('Unhandled Rejection:', reason);
});