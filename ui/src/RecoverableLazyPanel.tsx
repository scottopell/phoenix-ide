import { Component, Suspense, type ErrorInfo, type ReactNode } from 'react';

type Props = {
  children: ReactNode;
  onClose?: () => void;
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
    if (!this.state.failed) return this.props.children;
    if (!this.props.onClose) return null;

    return (
      <div className="lazy-panel-failure" role="alert">
        <span>This view could not be loaded.</span>
        <button className="lazy-panel-failure__close" type="button" onClick={this.props.onClose}>Return to conversation</button>
      </div>
    );
  }
}

export function RecoverableLazyPanel({ children, onClose }: Props) {
  return (
    <LazyPanelErrorBoundary {...(onClose ? { onClose } : {})}>
      <Suspense fallback={null}>{children}</Suspense>
    </LazyPanelErrorBoundary>
  );
}
