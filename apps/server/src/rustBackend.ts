/**
 * Rust backend proxy for Git Agent Harness
 * This bridges the TypeScript server with the existing Rust backend
 */

import { spawn, ChildProcess, ChildProcessWithoutNullStreams, SpawnOptions } from 'node:child_process';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { markReadinessCheck } from './serverReadiness.js';

const __dirname = fileURLToPath(new URL('.', import.meta.url));

class RustBackendProxy {
  private rustProcess: ChildProcess | null = null;
  private isReady = false;
  private rustBinPath: string;
  
  constructor() {
    // Try to find the Rust binary
    const possiblePaths = [
      resolve(__dirname, '../../../target/release/git-agent-harness'),
      resolve(__dirname, '../../../target/debug/git-agent-harness'),
      resolve(__dirname, '../../../target/release/gah'),
      resolve(__dirname, '../../../target/debug/gah'),
      'gah' // Try system PATH as fallback
    ];
    
    this.rustBinPath = possiblePaths[0]; // Default to release build
  }
  
  async start(): Promise<boolean> {
    try {
      // Try to find a working Rust binary
      let finalPath = this.rustBinPath;
      
      for (const path of [
        resolve(__dirname, '../../../target/release/gah'),
        resolve(__dirname, '../../../target/debug/gah'),
        'gah'
      ]) {
        try {
          // Test if the binary exists and is executable
          await import('node:fs').then(fs => fs.promises.access(path, fs.constants.X_OK));
          finalPath = path;
          break;
        } catch {
          // Try next path
        }
      }
      
      console.log(`Starting Rust backend from: ${finalPath}`);
      
      const options: SpawnOptions = {
        stdio: ['pipe', 'pipe', 'pipe'],
        cwd: resolve(__dirname, '..'),
        env: {
          ...process.env,
          RUST_LOG: process.env.RUST_LOG || 'info',
          GAH_SERVER_MODE: 'true' // Special mode for server integration
        }
      };
      
      this.rustProcess = spawn(finalPath, ['server'], options);
      
      // Handle stdout
      this.rustProcess.stdout?.on('data', (data) => {
        const message = data.toString().trim();
        if (message) {
          console.log(`[Rust] ${message}`);
          
          // Check for readiness message
          if (message.includes('Ready') || message.includes('Server started')) {
            this.isReady = true;
            markReadinessCheck('rustBackend', true);
          }
        }
      });
      
      // Handle stderr
      this.rustProcess.stderr?.on('data', (data) => {
        const message = data.toString().trim();
        if (message) {
          console.error(`[Rust Error] ${message}`);
          
          // Check for errors
          if (message.includes('error') || message.includes('Error')) {
            markReadinessCheck('rustBackend', false, message);
          }
        }
      });
      
      // Handle process exit
      this.rustProcess.on('exit', (code, signal) => {
        this.isReady = false;
        markReadinessCheck('rustBackend', false, `Process exited with code ${code} and signal ${signal}`);
        console.error(`Rust backend process exited with code ${code} and signal ${signal}`);
      });
      
      // Handle process error
      this.rustProcess.on('error', (error) => {
        this.isReady = false;
        markReadinessCheck('rustBackend', false, error.message);
        console.error('Rust backend process error:', error);
      });
      
      // Wait a bit for the process to start
      await new Promise(resolve => setTimeout(resolve, 2000));
      
      // Check if process is still running
      if (!this.rustProcess || this.rustProcess.exitCode !== null) {
        markReadinessCheck('rustBackend', false, 'Process failed to start');
        return false;
      }
      
      return true;
      
    } catch (error) {
      markReadinessCheck('rustBackend', false, String(error));
      console.error('Failed to start Rust backend:', error);
      return false;
    }
  }
  
  async stop(): Promise<void> {
    if (this.rustProcess && !this.rustProcess.killed) {
      this.rustProcess.kill('SIGTERM');
      
      // Wait for graceful shutdown
      await new Promise((resolve) => {
        const timeout = setTimeout(resolve, 5000);
        this.rustProcess?.on('exit', () => {
          clearTimeout(timeout);
          resolve(void 0);
        });
      });
      
      // Force kill if still running
      if (!this.rustProcess.killed) {
        this.rustProcess.kill('SIGKILL');
      }
    }
    
    this.rustProcess = null;
    this.isReady = false;
  }
  
  async sendCommand(command: string, args: string[] = []): Promise<string> {
    if (!this.isReady || !this.rustProcess) {
      throw new Error('Rust backend is not ready');
    }
    
    return new Promise((resolve, reject) => {
      const fullCommand = [command, ...args].join(' ');
      if (this.rustProcess?.stdin) {
        this.rustProcess.stdin.write(fullCommand + '\n', 'utf8', (error) => {
          if (error) {
            reject(error);
          } else {
            // For now, just resolve - we'll need proper JSON-RPC for real bidirectional communication
            resolve('Command sent');
          }
        });
      } else {
        reject(new Error('Rust backend stdin not available'));
      }
    });
  }
  
  isBackendReady(): boolean {
    return this.isReady;
  }
  
  getProcess(): ChildProcess | null {
    return this.rustProcess;
  }
}

const rustBackend = new RustBackendProxy();

export async function startRustBackendProxy(): Promise<void> {
  try {
    const success = await rustBackend.start();
    if (success) {
      console.log('Rust backend started successfully');
    } else {
      console.warn('Rust backend failed to start - some features may be limited');
      // Mark as ready anyway to allow the server to continue
      markReadinessCheck('rustBackend', true, 'Rust binary not found, running in limited mode');
    }
  } catch (error) {
    console.warn('Failed to start Rust backend:', error);
    markReadinessCheck('rustBackend', true, 'Rust backend unavailable, running in limited mode');
  }
}

export async function stopRustBackendProxy(): Promise<void> {
  await rustBackend.stop();
}

export function getRustBackendProxy(): RustBackendProxy {
  return rustBackend;
}

export { RustBackendProxy };