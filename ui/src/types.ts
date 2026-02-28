// Shared types

export interface Breadcrumb {
  type: 'user' | 'llm' | 'tool' | 'subagents';
  label: string;
  toolId?: string | undefined;
  sequenceId?: number | undefined;
  preview?: string | undefined;
}


