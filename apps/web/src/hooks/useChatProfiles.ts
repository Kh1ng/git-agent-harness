import { useEffect, useState } from 'react';
import { gahApi } from '../api/client.js';
import type { ChatProfile } from '../components/NewChatModal.js';

/**
 * Local configured profiles merged with imported remote projects, keyed by
 * chat profile name — the single project list every chat surface works
 * from. A failing half of the pair keeps its previous entries instead of
 * emptying the list.
 */
export function useChatProfiles(refreshKey: string | number = 0) {
  const [profiles, setProfiles] = useState<ChatProfile[]>([]);
  useEffect(() => {
    let cancelled = false;
    void Promise.allSettled([gahApi.getProfiles(), gahApi.getProjects()]).then(([local, catalog]) => {
      if (cancelled) return;
      setProfiles((previous) => {
        const localProfiles = local.status === 'fulfilled' ? local.value : previous.filter((item) => !item.remote);
        const projects = catalog.status === 'fulfilled'
          ? catalog.value.map((project) => ({
              ...project,
              name: project.chat_profile ?? project.name,
              catalogName: project.name,
              remote: !!project.chat_profile && project.chat_profile !== project.name
            }))
          : previous.filter((item) => item.remote);
        return [...new Map([...localProfiles, ...projects].map((project) => [project.name, project])).values()];
      });
    });
    return () => { cancelled = true; };
  }, [refreshKey]);
  return [profiles, setProfiles] as const;
}
