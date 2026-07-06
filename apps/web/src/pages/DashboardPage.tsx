import React from 'react';
import { useWebSocket } from '../ws/WebSocketContext.js';
import { SessionCard } from '../components/SessionCard.js';
import { ProviderStatusCard } from '../components/ProviderStatusCard.js';
import type { Session } from '@git-agent-harness/contracts';

type DashboardPageProps = {
  sessions: Session[];
  onSelectSession: (session: Session) => void;
  isConnected: boolean;
};

export function DashboardPage({ 
  sessions, 
  onSelectSession, 
  isConnected 
}: DashboardPageProps) {
  const {
    providers, 
    providerStatuses,
    profile,
    mergeRequests,
    availability,
    blockers,
    constraints,
    errors,
    recentLedger
  } = useWebSocket();

  return (
    <div className="space-y-6">
      <div className="flex justify-between items-center">
        <h2 className="text-2xl font-bold text-gray-900">Dashboard</h2>
        {profile && (
          <div className="text-sm text-gray-500">
            Profile: {profile}
          </div>
        )}
      </div>

      {/* Blockers and Constraints (from real GAH data) */}
      {(blockers.length > 0 || constraints.length > 0) && (
        <div className="bg-yellow-50 border border-yellow-200 rounded-lg p-4">
          <h3 className="text-lg font-semibold text-yellow-800 mb-2">
            ⚠️ Action Blocked
          </h3>
          
          {blockers.length > 0 && (
            <div className="mb-2">
              <h4 className="font-medium text-yellow-700">Blockers:</h4>
              <ul className="list-disc list-inside text-sm text-yellow-600">
                {blockers.map((blocker, index) => (
                  <li key={index}>{blocker.kind}: {blocker.message || blocker.reason || 'Unknown'}</li>
                ))}
              </ul>
            </div>
          )}
          
          {constraints.length > 0 && (
            <div>
              <h4 className="font-medium text-yellow-700">Constraints:</h4>
              <ul className="list-disc list-inside text-sm text-yellow-600">
                {constraints.map((constraint, index) => (
                  <li key={index}>{constraint.kind}: {constraint.reason || 'Unknown'}</li>
                ))}
              </ul>
            </div>
          )}
        </div>
      )}

      {/* Errors (from real GAH data) */}
      {errors.length > 0 && (
        <div className="bg-red-50 border border-red-200 rounded-lg p-4">
          <h3 className="text-lg font-semibold text-red-800 mb-2">
            ❌ Errors
          </h3>
          <ul className="list-disc list-inside text-sm text-red-600">
            {errors.map((error, index) => (
              <li key={index}>{error.subsystem}: {error.message}</li>
            ))}
          </ul>
        </div>
      )}

      <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
        {/* Active Sessions */}
        <div>
          <h3 className="text-lg font-semibold text-gray-900 mb-4">
            Active Sessions
          </h3>
          
          {sessions.length === 0 ? (
            <div className="card text-center py-8">
              <p className="text-gray-500">
                {isConnected ? 'No active sessions' : 'Connect to see active sessions'}
              </p>
            </div>
          ) : (
            <div className="space-y-4">
              {sessions.slice(0, 3).map(session => (
                <SessionCard 
                  key={session.id} 
                  session={session} 
                  onClick={() => onSelectSession(session)} 
                />
              ))}
            </div>
          )}
        </div>

        {/* Provider Status and Recent Ledger */}
        <div>
          <h3 className="text-lg font-semibold text-gray-900 mb-4">
            Provider Status
          </h3>
          
          <div className="space-y-3 max-h-48 overflow-y-auto">
            {providers.map(provider => (
              <ProviderStatusCard 
                key={provider.instanceId}
                provider={provider}
                status={providerStatuses[provider.instanceId]}
              />
            ))}
          </div>
          
          {/* Recent Ledger (from real GAH data) */}
          {recentLedger && (
            <div className="mt-6 p-4 bg-gray-50 rounded-lg">
              <h4 className="font-medium text-gray-700 mb-2">Recent Activity</h4>
              <div className="text-sm text-gray-600 space-y-1">
                {recentLedger.most_recent_mode && (
                  <div>Last mode: {recentLedger.most_recent_mode}</div>
                )}
                {recentLedger.most_recent_work_id && (
                  <div>Last work: {recentLedger.most_recent_work_id}</div>
                )}
                {recentLedger.most_recent_effective_backend && (
                  <div>Backend: {recentLedger.most_recent_effective_backend}</div>
                )}
                {recentLedger.most_recent_mr_url && (
                  <div>
                    <a 
                      href={recentLedger.most_recent_mr_url} 
                      target="_blank" 
                      rel="noopener noreferrer"
                      className="text-blue-600 hover:underline"
                    >
                      Last MR
                    </a>
                  </div>
                )}
                {recentLedger.human_required && (
                  <div className="text-red-500">⚠️ Human intervention required</div>
                )}
              </div>
            </div>
          )}
        </div>
      </div>

      {/* Merge Requests (from real GAH data) */}
      {mergeRequests.length > 0 && (
        <div className="mt-6">
          <h3 className="text-lg font-semibold text-gray-900 mb-4">
            Merge Requests
          </h3>
          <div className="overflow-x-auto">
            <table className="min-w-full divide-y divide-gray-200">
              <thead className="bg-gray-50">
                <tr>
                  <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                    Branch
                  </th>
                  <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                    State
                  </th>
                  <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                    Classification
                  </th>
                  <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                    Action
                  </th>
                  <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                    URL
                  </th>
                </tr>
              </thead>
              <tbody className="bg-white divide-y divide-gray-200">
                {mergeRequests.map((mr, index) => (
                  <tr key={index} className="hover:bg-gray-50">
                    <td className="px-6 py-4 whitespace-nowrap text-sm text-gray-900">
                      {mr.branch}
                    </td>
                    <td className="px-6 py-4 whitespace-nowrap text-sm text-gray-500">
                      {mr.state || 'Unknown'}
                    </td>
                    <td className="px-6 py-4 whitespace-nowrap text-sm text-gray-500">
                      {mr.classification}
                    </td>
                    <td className="px-6 py-4 whitespace-nowrap text-sm text-gray-500">
                      {mr.recommended_action}
                    </td>
                    <td className="px-6 py-4 whitespace-nowrap text-sm">
                      {mr.url && (
                        <a 
                          href={mr.url} 
                          target="_blank" 
                          rel="noopener noreferrer"
                          className="text-blue-600 hover:underline"
                        >
                          View MR
                        </a>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}

      {/* Availability (from real GAH data) */}
      {availability.length > 0 && (
        <div className="mt-6">
          <h3 className="text-lg font-semibold text-gray-900 mb-4">
            Backend Availability
          </h3>
          <div className="overflow-x-auto">
            <table className="min-w-full divide-y divide-gray-200">
              <thead className="bg-gray-50">
                <tr>
                  <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                    Backend
                  </th>
                  <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                    Model
                  </th>
                  <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                    Status
                  </th>
                  <th className="px-6 py-3 text-left text-xs font-medium text-gray-500 uppercase tracking-wider">
                    Reason
                  </th>
                </tr>
              </thead>
              <tbody className="bg-white divide-y divide-gray-200">
                {availability.map((avail, index) => (
                  <tr key={index} className={avail.eligible_now ? 'hover:bg-gray-50' : 'bg-red-50 hover:bg-red-100'}>
                    <td className="px-6 py-4 whitespace-nowrap text-sm text-gray-900">
                      {avail.backend}
                    </td>
                    <td className="px-6 py-4 whitespace-nowrap text-sm text-gray-500">
                      {avail.model || 'N/A'}
                    </td>
                    <td className="px-6 py-4 whitespace-nowrap text-sm">
                      {avail.eligible_now ? (
                        <span className="text-green-600">✓ Available</span>
                      ) : (
                        <span className="text-red-600">✗ Unavailable</span>
                      )}
                    </td>
                    <td className="px-6 py-4 whitespace-nowrap text-sm text-gray-500">
                      {avail.reason || 'N/A'}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      )}
    </div>
  );
}