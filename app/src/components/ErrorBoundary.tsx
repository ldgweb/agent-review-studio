import { Component, type ErrorInfo, type ReactNode } from 'react';
import { openExternal } from '../lib/desktop';

// Last line of defense: a crash in any view must never leave the user with
// a white window. Shows the error, offers reload and a copyable report.

const ISSUES_URL = 'https://github.com/ldgweb/agent-review-studio/issues/new';

interface State {
  error: Error | null;
  detail: string;
  /** Opening the issue tracker failed */
  linkError: string;
}

export class ErrorBoundary extends Component<{ children: ReactNode }, State> {
  state: State = { error: null, detail: '', linkError: '' };

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    const detail = `${error.stack ?? error.message}\n\nComponent stack:${info.componentStack ?? ''}`;
    this.setState({ detail });
    console.error('[ars] crash', error, info);
  }

  render() {
    if (!this.state.error) return this.props.children;
    return (
      <main className="page">
        <div className="card" style={{ maxWidth: 680 }}>
          <div style={{ fontSize: 'var(--fs-lg)', fontWeight: 700, color: 'var(--err)' }}>
            Something broke in the interface
          </div>
          <p className="muted mt-2">
            Saved sessions and reviews are stored locally. Recent unsaved decisions may need to be repeated.
            Reload to continue. Remove private paths and content from copied details before reporting an issue.
          </p>
          <pre
            className="mono"
            style={{
              marginTop: 'var(--sp-3)',
              padding: 'var(--sp-3)',
              background: 'var(--bg-0)',
              border: '1px solid var(--border-0)',
              borderRadius: 'var(--radius-md)',
              fontSize: 'var(--fs-xs)',
              maxHeight: 220,
              overflow: 'auto',
              whiteSpace: 'pre-wrap',
            }}
          >
            {String(this.state.error)}
          </pre>
          <div className="flex mt-3">
            <button className="btn btn-primary" onClick={() => window.location.reload()}>
              Reload
            </button>
            <button
              className="btn"
              onClick={() => navigator.clipboard.writeText(this.state.detail).catch(() => undefined)}
            >
              Copy details
            </button>
            <button
              className="btn"
              onClick={() =>
                openExternal(ISSUES_URL).catch((e: unknown) =>
                  this.setState({ linkError: `Could not open ${ISSUES_URL}: ${e instanceof Error ? e.message : String(e)}` }),
                )
              }
            >
              Report issue
            </button>
          </div>
          {this.state.linkError && (
            <div className="mt-2" role="alert" style={{ color: 'var(--err)', fontSize: 'var(--fs-sm)' }}>
              {this.state.linkError}
            </div>
          )}
        </div>
      </main>
    );
  }
}
