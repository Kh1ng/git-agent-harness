import { useEffect } from 'react';
import type { StatusSnapshot } from '@git-agent-harness/contracts';
import { useGahStore } from '../store/gahStore.js';

export function WorkerSnapshotProbe() {
  const status = useGahStore(state => state.status);
  useEffect(() => {
    useGahStore.setState({ status: {
      data: { profile: { profile: 'gah' }, running_workers: [] } as unknown as StatusSnapshot,
      key: 'gah', loading: false, error: null, fetchedAt: null,
    } });
  }, []);
  return <div>
    <button onClick={() => void useGahStore.getState().fetchStatus('gah', { force: true })}>Fetch status</button>
    <span>{status.loading ? 'Loading' : 'Settled'}</span>
    <span>{status.error}</span>
    <span>Workers: {status.data?.running_workers?.map(worker => worker.run_id).join(',')}</span>
  </div>;
}
