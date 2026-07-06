import React, { useState, useEffect } from 'react';
import ReactMarkdown from 'react-markdown';
import type { Session } from '@git-agent-harness/contracts';

type SessionDetailModalProps = {
  session: Session;
  onClose: () => void;
};

const statusColors: Record<string, string> = {
  idle: 'bg-gray-100 text-gray-800',
  starting: 'bg-yellow-100 text-yellow-800',
  running: 'bg-green-100 text-green-800',
  stopping: 'bg-yellow-100 text-yellow-800',
  stopped: 'bg-gray-100 text-gray-800',
  error: 'bg-red-100 text-red-800',
};

const providerIcons: Record<string, string> = {
  github: '💻',
  gitlab: '🦊',
  codex: '🤖',
  claude: '🎯',
  cursor: '✨',
  opencode: '🔓',
  grok: '🌟',
  openhands: '🤝',
  agy: '🧠',
  vibe: '⚡',
  auto: '🎲',
};

export function SessionDetailModal({ session, onClose }: SessionDetailModalProps) {
  const [output, setOutput] = useState<string>('');
  const statusColor = statusColors[session.status] || 'bg-gray-100 text-gray-800';
  const providerIcon = providerIcons[session.providerKind] || '📦';

  // Simulate output updates
  useEffect(() => {
    // In a real implementation, this would subscribe to session output updates
    const interval = setInterval(() => {
      if (session.status === 'running') {
        setOutput(prev => prev + `Session ${session.id} is running...\n`);
      }
    }, 3000);
    
    return () => clearInterval(interval);
  }, [session.id, session.status]);

  const handleStopSession = () => {
    // Send stop command via WebSocket
    // This would be implemented in a real version
    alert(`Would stop session ${session.id}`);
    onClose();
  };

  const handleSendCommand = () => {
    const command = prompt('Enter command:');
    if (command) {
      // Send command via WebSocket
      alert(`Would send command: ${command}`);
    }
  };

  return (
    <div className="fixed inset-0 bg-black bg-opacity-50 flex items-center justify-center p-4 z-50">
      <div className="bg-white rounded-lg shadow-xl max-w-4xl w-full max-h-[90vh] overflow-hidden flex flex-col">
        <div className="flex justify-between items-center p-6 border-b border-gray-200">
          <div className="flex items-center space-x-3">
            <span className="text-2xl">{providerIcon}</span>
            <div>
              <h3 className="text-lg font-semibold text-gray-900">
                Session Details
              </h3>
              <p className="text-sm text-gray-500">{session.id}</p>
            </div>
          </div>
          
          <div className="flex items-center space-x-3">
            <span className={`badge ${statusColor}`}>
              {session.status}
            </span>
            <button 
              onClick={onClose}
              className="text-gray-400 hover:text-gray-600"
            >
              ×
            </button>
          </div>
        </div>

        <div className="flex-1 overflow-y-auto p-6">
          <div className="grid grid-cols-1 md:grid-cols-2 gap-6 mb-6">
            <div>
              <h4 className="text-sm font-medium text-gray-500 mb-2">Repository</h4>
              <p className="text-gray-900">{session.repo}</p>
            </div>
            
            <div>
              <h4 className="text-sm font-medium text-gray-500 mb-2">Provider</h4>
              <p className="text-gray-900">{session.providerKind}</p>
            </div>
            
            <div>
              <h4 className="text-sm font-medium text-gray-500 mb-2">Mode</h4>
              <p className="text-gray-900">{session.mode}</p>
            </div>
            
            <div>
              <h4 className="text-sm font-medium text-gray-500 mb-2">Backend</h4>
              <p className="text-gray-900">{session.backend || 'N/A'}</p>
            </div>
            
            {session.branch && (
              <div>
                <h4 className="text-sm font-medium text-gray-500 mb-2">Branch</h4>
                <p className="text-gray-900">{session.branch}</p>
              </div>
            )}
            
            {session.target && (
              <div>
                <h4 className="text-sm font-medium text-gray-500 mb-2">Target</h4>
                <p className="text-gray-900">{session.target}</p>
              </div>
            )}
            
            {session.model && (
              <div>
                <h4 className="text-sm font-medium text-gray-500 mb-2">Model</h4>
                <p className="text-gray-900">{session.model}</p>
              </div>
            )}
            
            {session.budget && (
              <div>
                <h4 className="text-sm font-medium text-gray-500 mb-2">Budget</h4>
                <p className="text-gray-900">{session.budget}</p>
              </div>
            )}
            
            <div>
              <h4 className="text-sm font-medium text-gray-500 mb-2">Started</h4>
              <p className="text-gray-900">{session.startedAt || 'N/A'}</p>
            </div>
            
            {session.endedAt && (
              <div>
                <h4 className="text-sm font-medium text-gray-500 mb-2">Ended</h4>
                <p className="text-gray-900">{session.endedAt}</p>
              </div>
            )}
          </div>

          {session.error && (
            <div className="mb-6 p-4 bg-red-50 rounded-lg">
              <h4 className="text-sm font-medium text-red-800 mb-2">Error</h4>
              <ReactMarkdown className="text-red-700 text-sm">{session.error}</ReactMarkdown>
            </div>
          )}

          <div className="mb-4">
            <h4 className="text-sm font-medium text-gray-500 mb-2">Output</h4>
            <div className="terminal max-h-64 overflow-y-auto">
              <pre className="whitespace-pre-wrap">{output || 'Session output will appear here...'}</pre>
            </div>
          </div>
        </div>

        <div className="flex justify-end space-x-3 p-6 border-t border-gray-200 bg-gray-50">
          {session.status === 'running' && (
            <button 
              onClick={handleSendCommand}
              className="btn btn-secondary"
            >
              Send Command
            </button>
          )}
          
          {session.status === 'running' && (
            <button 
              onClick={handleStopSession}
              className="btn btn-danger"
            >
              Stop Session
            </button>
          )}
          
          <button 
            onClick={onClose}
            className="btn btn-secondary"
          >
            Close
          </button>
        </div>
      </div>
    </div>
  );
}