import { lazy, Suspense, useState } from 'react';
import { HashRouter, Navigate, NavLink, Route, Routes, useLocation, useNavigate, useSearchParams } from 'react-router-dom';
import { getTheme, toggleTheme, type Theme } from './lib/theme';
import { SessionsProvider } from './state/sessionsStore';
import { ProjectsHome } from './features/home/ProjectsHome';
import { ErrorBoundary } from './components/ErrorBoundary';
import { IconArrowLeft, IconSettings } from './components/icons';
import { closeWindow, isDesktop, minimizeWindow, toggleMaximizeWindow } from './lib/desktop';

// Code splitting by route: the projects home is the first paint and stays
// in the main chunk; the review (the diff viewer and its grammars) and
// Settings load when first opened.
const ReviewPage = lazy(() => import('./features/review/ReviewPage').then((m) => ({ default: m.ReviewPage })));
const SettingsView = lazy(() =>
  import('./features/settings/SettingsView').then((m) => ({ default: m.SettingsView })),
);

// One quiet bar owns all app chrome: the brand (home), where you are, and
// settings. The page below spends its pixels on the code.

function ProjectCrumb() {
  const [params] = useSearchParams();
  const path = params.get('path') ?? '';
  const name = path.replace(/[\\/]+$/, '').split(/[\\/]/).pop() ?? path;
  return (
    <div className="crumb">
      <NavLink to="/" className="btn btn-sm crumb-back" title="All projects" aria-label="All projects">
        <IconArrowLeft size={13} />
      </NavLink>
      <span className="crumb-goal" title={path}>{name}</span>
    </div>
  );
}

/** Settings: back to where you came from (the projects, when it opened first) */
function SettingsCrumb() {
  const navigate = useNavigate();
  const back = () => {
    const idx = (window.history.state as { idx?: number } | null)?.idx ?? 0;
    if (idx > 0) navigate(-1);
    else navigate('/');
  };
  return (
    <div className="crumb">
      <button className="btn btn-sm crumb-back" title="Back" aria-label="Back" onClick={back}>
        <IconArrowLeft size={13} />
      </button>
      <span className="crumb-goal">Settings</span>
    </div>
  );
}

function ThemeToggle() {
  const [theme, setTheme] = useState<Theme>(getTheme);
  return (
    <button
      className="icon-btn"
      title={theme === 'dark' ? 'Switch to light theme' : 'Switch to dark theme'}
      aria-label={theme === 'dark' ? 'Switch to light theme' : 'Switch to dark theme'}
      onClick={() => setTheme(toggleTheme())}
    >
      {theme === 'dark' ? '☾' : '☀'}
    </button>
  );
}

function TopBar() {
  const desktop = isDesktop();
  const { pathname } = useLocation();

  return (
    <header className={`topbar${desktop ? ' topbar-desktop' : ''}`} data-tauri-drag-region="">
      <NavLink to="/" className="topbar-brand" title="All projects">
        <span>A</span>RS
      </NavLink>
      {pathname === '/project' && <ProjectCrumb />}
      {pathname === '/settings' && <SettingsCrumb />}
      <div className="topbar-spacer" data-tauri-drag-region="" />
      <NavLink to="/settings" className={({ isActive }) => `icon-btn${isActive ? ' active' : ''}`} title="Settings" aria-label="Settings">
        <IconSettings size={15} />
      </NavLink>
      <ThemeToggle />
      {desktop && (
        <div className="win-controls">
          <button className="win-btn" title="Minimize" aria-label="Minimize" onClick={minimizeWindow}>
            <svg width="10" height="10" viewBox="0 0 10 10"><line x1="0" y1="5" x2="10" y2="5" stroke="currentColor" strokeWidth="1.2" /></svg>
          </button>
          <button className="win-btn" title="Maximize" aria-label="Maximize" onClick={toggleMaximizeWindow}>
            <svg width="10" height="10" viewBox="0 0 10 10"><rect x="0.8" y="0.8" width="8.4" height="8.4" fill="none" stroke="currentColor" strokeWidth="1.2" /></svg>
          </button>
          <button className="win-btn win-btn-close" title="Close" aria-label="Close" onClick={closeWindow}>
            <svg width="10" height="10" viewBox="0 0 10 10"><path d="M0 0 L10 10 M10 0 L0 10" stroke="currentColor" strokeWidth="1.2" /></svg>
          </button>
        </div>
      )}
    </header>
  );
}

export function App() {
  return (
    <SessionsProvider>
      <HashRouter>
        <ErrorBoundary>
          <TopBar />
          <Suspense fallback={<main className="page"><span className="spinner" /></main>}>
            <Routes>
              <Route path="/" element={<ProjectsHome />} />
              <Route path="/settings" element={<SettingsView />} />
              {/* `ars <dir>` and the projects home land here */}
              <Route path="/project" element={<ReviewPage />} />
              <Route path="*" element={<Navigate to="/" replace />} />
            </Routes>
          </Suspense>
        </ErrorBoundary>
      </HashRouter>
    </SessionsProvider>
  );
}
