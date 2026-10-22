import { lazy, Suspense, useState } from 'react';
import { HashRouter, Navigate, NavLink, Route, Routes, useMatch } from 'react-router-dom';
import { getTheme, toggleTheme, type Theme } from './lib/theme';
import { ApprovalProvider, effectiveStatus, useApproval } from './state/approvalStore';
import { SessionsProvider, useSessions } from './state/sessionsStore';
import { SessionsInbox } from './features/inbox/SessionsInbox';
import { AgentBadge, StatusPill } from './components/ui';
import { ErrorBoundary } from './components/ErrorBoundary';
import { IconArrowLeft } from './components/icons';
import { closeWindow, isDesktop, minimizeWindow, toggleMaximizeWindow } from './lib/desktop';

// Route-level code splitting: the inbox is the first paint and stays in the
// main chunk; the session workbench (which carries DiffView + the refractor
// grammars) and Settings load on first navigation.
const SessionLayout = lazy(() =>
  import('./features/session/SessionLayout').then((m) => ({ default: m.SessionLayout })),
);
const ReviewWorkbench = lazy(() =>
  import('./features/session/tabs/ReviewWorkbench').then((m) => ({ default: m.ReviewWorkbench })),
);
const SettingsView = lazy(() =>
  import('./features/settings/SettingsView').then((m) => ({ default: m.SettingsView })),
);

// One 40px bar owns all app chrome. On session routes it becomes the session
// header too: back, agent, goal, status — the page below spends its pixels
// on the code.

function SessionCrumb({ id }: { id: string }) {
  const { getSession, getSessionDetail } = useSessions();
  const session = getSessionDetail(id) ?? getSession(id);
  const { state } = useApproval(id, session?.reviewRevision, session?.contentHash);
  if (!session) return null;

  return (
    <div className="crumb">
      <NavLink to="/" className="btn btn-sm crumb-back" title="Back to sessions">
        <IconArrowLeft size={13} />
      </NavLink>
      <AgentBadge agent={session.agent} label={session.agentLabel} />
      <span className="crumb-goal" title={`${session.declaredGoal}\n${session.projectName} @ ${session.branch} · ${session.model}`}>
        {session.declaredGoal}
      </span>
      <StatusPill status={effectiveStatus(session, state)} />
    </div>
  );
}

function ThemeToggle() {
  const [theme, setTheme] = useState<Theme>(getTheme);
  return (
    <button
      className="btn btn-sm theme-toggle"
      title={theme === 'dark' ? 'Switch to light theme' : 'Switch to dark theme'}
      aria-label="Toggle light or dark theme"
      onClick={() => setTheme(toggleTheme())}
    >
      {theme === 'dark' ? '☾' : '☀'}
    </button>
  );
}

function TopBar() {
  const desktop = isDesktop();
  const match = useMatch('/sessions/:id/*');
  // React Router already decodes params — the same id SessionLayout reads
  const sessionId = match?.params.id ?? null;

  return (
    <header className={`topbar${desktop ? ' topbar-desktop' : ''}`} data-tauri-drag-region="">
      <span className="topbar-brand" data-tauri-drag-region="" title="Agent Review Studio">
        <span>A</span>RS
      </span>
      <nav className="topnav">
        <NavLink to="/" end className={({ isActive }) => (isActive ? 'active' : '')}>
          Sessions
        </NavLink>
        <NavLink to="/settings" className={({ isActive }) => (isActive ? 'active' : '')}>
          Settings
        </NavLink>
      </nav>
      {sessionId && <SessionCrumb id={sessionId} />}
      <div className="topbar-spacer" data-tauri-drag-region="" />
      <ThemeToggle />
      {desktop && (
        <div className="win-controls">
          <button className="win-btn" title="Minimize" onClick={minimizeWindow}>
            <svg width="10" height="10" viewBox="0 0 10 10"><line x1="0" y1="5" x2="10" y2="5" stroke="currentColor" strokeWidth="1.2" /></svg>
          </button>
          <button className="win-btn" title="Maximize" onClick={toggleMaximizeWindow}>
            <svg width="10" height="10" viewBox="0 0 10 10"><rect x="0.8" y="0.8" width="8.4" height="8.4" fill="none" stroke="currentColor" strokeWidth="1.2" /></svg>
          </button>
          <button className="win-btn win-btn-close" title="Close" onClick={closeWindow}>
            <svg width="10" height="10" viewBox="0 0 10 10"><path d="M0 0 L10 10 M10 0 L0 10" stroke="currentColor" strokeWidth="1.2" /></svg>
          </button>
        </div>
      )}
    </header>
  );
}

export function App() {
  return (
    <ApprovalProvider>
      <SessionsProvider>
      <HashRouter>
        <ErrorBoundary>
        <TopBar />
        <Suspense fallback={<main className="page"><span className="spinner" /></main>}>
        <Routes>
          <Route path="/" element={<SessionsInbox />} />
          <Route path="/settings" element={<SettingsView />} />
          <Route path="/sessions/:id" element={<SessionLayout />}>
            <Route index element={<Navigate to="review" replace />} />
            <Route path="review" element={<ReviewWorkbench />} />
            {/* Unknown session sub-routes land on the review */}
            <Route path="*" element={<Navigate to="../review" replace />} />
          </Route>
          <Route path="*" element={<Navigate to="/" replace />} />
        </Routes>
        </Suspense>
        </ErrorBoundary>
      </HashRouter>
      </SessionsProvider>
    </ApprovalProvider>
  );
}
