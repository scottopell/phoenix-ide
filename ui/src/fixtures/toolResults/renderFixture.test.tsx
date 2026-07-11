import { afterEach, describe, expect, it, vi } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import { ToolResultsFixture } from './renderFixture';
import { getToolResultsScenario } from './scenarios';

vi.mock('../../components/MessageContextMenu', () => ({
  MessageContextMenu: () => null,
}));

vi.mock('../../components/FilePathContextMenu', () => ({
  FilePathContextMenu: () => null,
}));

afterEach(() => {
  cleanup();
});

describe('ToolResultsFixture', () => {
  it('renders every full shell tool result without virtualization hiding earlier cases', async () => {
    const { container } = render(<ToolResultsFixture scenario={getToolResultsScenario('shell-full')} />);

    await screen.findByText('Tool results fixture');

    expect(screen.getByText('Tool results fixture')).toBeInTheDocument();
    expect(screen.getByText('family=shell')).toBeInTheDocument();
    expect(screen.getByText('density=full')).toBeInTheDocument();
    expect(screen.getByText(/This shell family is the transcript-level smoke test/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'thinking (1 line)' })).toBeInTheDocument();
    expect(screen.getByText(/Navigation complete — fixture page ready/)).toBeInTheDocument();
    expect(screen.getByText('resize 390x844')).toBeInTheDocument();
    expect(screen.getByText(/Which tool-result family should we refine next/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Review' })).toBeInTheDocument();
    expect(container.querySelector('[data-tool-results-fixture-ready="shell-full"]')).not.toBeNull();
    expect(screen.getByText(/120 more returned lines/)).toBeInTheDocument();
    expect(screen.getByText(/earlier click is a finalized missing result/)).toBeInTheDocument();
    expect(container.querySelectorAll('.tool-missing-result')).toHaveLength(1);
    expect(container.querySelector('[data-tool-id="shell-pending"]')).not.toBeNull();
    expect(container.querySelector('[data-tool-id="shell-pending"] .tool-missing-result')).toBeNull();
    expect(container.querySelector('[data-tool-id="shell-pending"] .tool-block-elapsed')?.textContent).toMatch(/• [4-9]s/);
    expect(screen.getByText('custom_fixture_tool')).toBeInTheDocument();
  });

  it('renders compact execution summaries while keeping interactive execution renderer affordances', async () => {
    const { container } = render(<ToolResultsFixture scenario={getToolResultsScenario('execution-compact')} />);

    await screen.findByText('density=compact');

    expect(screen.getByText('density=compact')).toBeInTheDocument();
    expect(screen.getByText('agent-browser')).toBeInTheDocument();
    expect(screen.getByText('running')).toBeInTheDocument();
    expect(screen.getByText(/This family deliberately mixes typed payloads/)).toBeInTheDocument();
    expect(screen.getByText(/without a live backend/)).toBeInTheDocument();
    expect(container.querySelectorAll('.compact-tool-card').length).toBeGreaterThan(0);
    expect(container.querySelector('[data-tool-results-fixture-ready="execution-compact"]')).not.toBeNull();
  });

  it('renders the grouped specialized renderer families for discovery, media, profiling, and subagents', async () => {
    const { rerender } = render(<ToolResultsFixture scenario={getToolResultsScenario('discovery-full')} />);

    await screen.findByText('3 relevant files');
    expect(screen.getByText(/deterministicFixtureLine1 = true/)).toBeInTheDocument();
    expect(screen.getByText('ui/src/fixtures/toolResults/scenarios.ts')).toBeInTheDocument();
    expect(screen.getByText(/12 lines • lines 1-12/)).toBeInTheDocument();
    expect(screen.getByText(/4 more returned lines/)).toBeInTheDocument();
    expect(screen.getByText('(empty file)')).toBeInTheDocument();
    expect(screen.getByText(/Ignored 1 non-numbered line/)).toBeInTheDocument();

    rerender(<ToolResultsFixture scenario={getToolResultsScenario('media-full')} />);
    await screen.findByText('scenario=media-full');
    expect(await screen.findByText('3 entries')).toBeInTheDocument();
    expect(screen.getAllByAltText('Tool result')).toHaveLength(3);

    rerender(<ToolResultsFixture scenario={getToolResultsScenario('profiling-full')} />);
    await screen.findByText('scenario=profiling-full');
    expect(await screen.findByText('✓ completed')).toBeInTheDocument();
    expect(screen.getAllByText('CPU profile').length).toBeGreaterThanOrEqual(2);
    expect(screen.getByText('trace')).toBeInTheDocument();
    expect(screen.getByText('heap diff')).toBeInTheDocument();
    expect(screen.getByText('ScriptDuration')).toBeInTheDocument();
    expect(screen.getByText(/browser_profile run_scenario failed: page crashed/)).toBeInTheDocument();
    expect(screen.queryByText('unknown')).not.toBeInTheDocument();

    rerender(<ToolResultsFixture scenario={getToolResultsScenario('subagents-full')} />);
    await screen.findByText('scenario=subagents-full');
    expect(await screen.findByText('completed')).toBeInTheDocument();
    expect(screen.getByText(/The spawn_agents fixture uses the approved tasks-shaped input/)).toBeInTheDocument();
  });
});
