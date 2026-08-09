import { Component, Suspense, type ErrorInfo, type ReactNode } from 'react';

type Props = {
  children: ReactNode;
};

type State = {
  failed: boolean;
};

class LazyPanelErrorBoundary extends Component<Props, State> {
  override state: State = { failed: false };

  static getDerivedStateFromError(): State {
    return { failed: true };
  }

  override componentDidCatch(error: unknown, info: ErrorInfo): void {
    console.error('[Phoenix] A nested lazy panel failed to initialize.', error, info.componentStack);
  }

  override render(): ReactNode {
    return this.state.failed ? null : this.props.children;
  }
}

export function RecoverableLazyPanel({ children }: Props) {
  return (
    <LazyPanelErrorBoundary>
      <Suspense fallback={null}>{children}</Suspense>
    </LazyPanelErrorBoundary>
  );
}
