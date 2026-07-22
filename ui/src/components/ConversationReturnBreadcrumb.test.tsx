import { fireEvent, render, screen } from '@testing-library/react';
import { MemoryRouter, Route, Routes, useLocation } from 'react-router-dom';
import { describe, expect, it } from 'vitest';
import { ConversationReturnBreadcrumb } from './ConversationReturnBreadcrumb';
import type { ConversationLinkState } from './conversationReturnOrigin';

function CurrentPath() {
  const location = useLocation();
  return <output>{location.pathname}</output>;
}

describe('ConversationReturnBreadcrumb', () => {
  it('reuses the seeded-conversation breadcrumb presentation to return to Coordinator', () => {
    const state: ConversationLinkState = {
      conversationReturnOrigin: { kind: 'coordinator', href: '/global/coordinator-id' },
    };
    render(
      <MemoryRouter initialEntries={[{ pathname: '/c/source', state }]}>
        <ConversationReturnBreadcrumb />
        <CurrentPath />
      </MemoryRouter>,
    );

    const link = screen.getByRole('link', { name: '← from: Coordinator' });
    expect(link.closest('.conversation-seed-breadcrumb')).not.toBeNull();
    fireEvent.click(link);
    expect(screen.getByText('/global/coordinator-id')).toBeInTheDocument();
  });

  it('does not render without a valid Coordinator return origin', () => {
    render(
      <MemoryRouter initialEntries={['/c/source']}>
        <Routes><Route path="*" element={<ConversationReturnBreadcrumb />} /></Routes>
      </MemoryRouter>,
    );

    expect(screen.queryByText(/from: Coordinator/)).not.toBeInTheDocument();
  });
});
