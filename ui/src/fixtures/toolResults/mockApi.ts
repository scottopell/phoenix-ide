import { api } from '../../api';

export function installToolResultsFixtureApi() {
  const original = api.listForkProposals;
  api.listForkProposals = async () => [{
    id: 'fixture-fork-proposal',
    status: 'pending',
    title: 'Refine structured tool-result rendering',
    priority: 'p2',
    task_file: 'tasks/fixture-tool-results.md',
    body: 'Use the executable fixture matrix to replace generic JSON fallbacks.',
  }];
  return () => {
    api.listForkProposals = original;
  };
}
