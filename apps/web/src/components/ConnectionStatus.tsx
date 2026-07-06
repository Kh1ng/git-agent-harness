import React from 'react';

type ConnectionStatusProps = {
  isConnected: boolean;
  isConnecting: boolean;
  error: string | null;
  serverVersion: string | null;
};

export function ConnectionStatus({ 
  isConnected, 
  isConnecting, 
  error, 
  serverVersion 
}: ConnectionStatusProps) {
  if (isConnecting) {
    return (
      <div className="rounded-md bg-yellow-50 p-4">
        <div className="flex">
          <div className="flex-shrink-0">
            <div className="h-5 w-5 rounded-full bg-yellow-400 animate-pulse"></div>
          </div>
          <div className="ml-3">
            <p className="text-sm font-medium text-yellow-800">
              Connecting to server...
            </p>
          </div>
        </div>
      </div>
    );
  }

  if (error) {
    return (
      <div className="rounded-md bg-red-50 p-4">
        <div className="flex">
          <div className="flex-shrink-0">
            <div className="h-5 w-5 rounded-full bg-red-400"></div>
          </div>
          <div className="ml-3">
            <p className="text-sm font-medium text-red-800">
              Connection error: {error}
            </p>
          </div>
        </div>
      </div>
    );
  }

  if (!isConnected) {
    return (
      <div className="rounded-md bg-gray-50 p-4">
        <div className="flex">
          <div className="flex-shrink-0">
            <div className="h-5 w-5 rounded-full bg-gray-400"></div>
          </div>
          <div className="ml-3">
            <p className="text-sm font-medium text-gray-800">
              Disconnected from server
            </p>
          </div>
        </div>
      </div>
    );
  }

  return (
    <div className="rounded-md bg-green-50 p-4">
      <div className="flex items-center justify-between">
        <div className="flex items-center">
          <div className="flex-shrink-0">
            <div className="h-5 w-5 rounded-full bg-green-400 animate-pulse"></div>
          </div>
          <div className="ml-3">
            <p className="text-sm font-medium text-green-800">
              Connected to server
              {serverVersion && (
                <span className="ml-2 text-sm font-normal text-green-600">
                  v{serverVersion}
                </span>
              )}
            </p>
          </div>
        </div>
      </div>
    </div>
  );
}