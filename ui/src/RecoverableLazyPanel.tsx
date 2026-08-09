import { Component, Suspense, type ErrorInfo, type ReactNode } from 'react';
import { isModuleAcquisitionFailure } from './moduleAcquisitionFailure';
import './RecoverableLazyPanel.css';

type Props = {
  children: ReactNode;
  onClose?: () => void;
};

type State = {
  moduleFailed: boolean;
  unexpectedError?: unknown;
};

class LazyPanelErrorBoundary extends Component<Props, State> {
  override state: State = { moduleFailed: false };

  static getDerivedStateFromError(error: unknown): State {
    return isModuleAcquisitionFailure(error)
      ? { moduleFailed: true }
      : { moduleFailed: false, unexpectedError: error };
  }

  override componentDidCatch(error: unknown, info: ErrorInfo): void {
    console.error('[Phoenix] A nested lazy panel failed to initialize.', error, info.componentStack);
  }

  override render(): ReactNode {
    if (this.state.unexpectedError !== undefined) throw this.state.unexpectedError;
    if (!this.state.moduleFailed) return this.props.children;
    if (!this.props.onClose) {
      return <div className="lazy-panel-failure" role="alert">This feature could not be loaded. Reload Phoenix when ready.</div>;
    }

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
