import type { AgentSession, FindingReviewState, RiskDimension } from '../../../types/domain';
import { IconX } from '../../../components/icons';
import { FindingsPanel } from './FindingsPanel';
import { GitTab } from './GitTab';
import { SessionPanel } from './SessionPanel';
import { TimelinePanel } from './TimelinePanel';

// The context rail — the single home for everything that is not the code:
// findings, git, the session summary and the timeline. One
// tab open at a time; badges surface what needs attention without stealing
// space from the diff.

export type RailTab = 'git' | 'findings' | 'session' | 'timeline';

// Findings first (what's flagged, at a glance), then Git, then Session (a
// prompt-focused read of the run), then Timeline.
export const RAIL_TABS: RailTab[] = ['findings', 'git', 'session', 'timeline'];

/** Label + badge for one rail tab — shared between the open rail's tab row
 * and the collapsed strip, so a closed panel still SHOWS what's waiting
 * (e.g. Findings · 2 in the findings amber, red when something is high). */
function railTabMeta(session: AgentSession, t: RailTab): { label: string; badge?: string; alert?: boolean } {
  switch (t) {
    case 'git':
      return { label: 'Git' };
    case 'findings': {
      if (session.findings.length === 0) return { label: 'Findings' };
      const hasHigh = session.findings.some((f) => f.severity === 'high' || f.severity === 'critical');
      return { label: 'Findings', badge: String(session.findings.length), alert: hasHigh };
    }
    case 'session':
      return { label: 'Session' };
    case 'timeline':
      return { label: 'Timeline' };
  }
}

/** Collapsed-rail affordance: a labelled vertical strip so it's obvious the
 * context panel exists, which tabs it holds — and what's waiting inside. */
export function RailStrip({
  session,
  current,
  onOpen,
}: {
  session: AgentSession;
  current: RailTab;
  onOpen: (tab: RailTab) => void;
}) {
  return (
    <div className="rail-strip">
      <button className="rail-open-btn" title="Open the context panel (x)" onClick={() => onOpen(current)}>
        ‹
      </button>
      {RAIL_TABS.map((t) => {
        const meta = railTabMeta(session, t);
        return (
          <button
            key={t}
            className="rail-strip-tab"
            onClick={() => onOpen(t)}
            title={meta.badge ? `Open ${meta.label} (${meta.badge})` : `Open ${meta.label}`}
          >
            <span className="strip-label">{meta.label}</span>
            {meta.badge && <span className={`rail-strip-badge${meta.alert ? ' alert' : ''}`}>{meta.badge}</span>}
          </button>
        );
      })}
    </div>
  );
}

interface ContextRailProps {
  session: AgentSession;
  tab: RailTab;
  onTab: (tab: RailTab) => void;
  findingStates: Record<string, FindingReviewState>;
  onSetFindingState: (findingId: string, state?: FindingReviewState) => void;
  onJumpToFile: (fileId: string, line?: number) => void;
  rejectedCategories: RiskDimension[];
  onRejectCategory: (dimension: RiskDimension, fileIds: string[]) => void;
  onClearCategory: (dimension: RiskDimension, fileIds: string[]) => void;
  approvedPaths: string[];
  onGitChanged: () => void;
  onClose: () => void;
}

export function ContextRail({
  session,
  tab,
  onTab,
  findingStates,
  onSetFindingState,
  onJumpToFile,
  rejectedCategories,
  onRejectCategory,
  onClearCategory,
  approvedPaths,
  onGitChanged,
  onClose,
}: ContextRailProps) {
  return (
    <aside className={`wb-rail${tab === 'session' || tab === 'timeline' || tab === 'git' ? ' wide' : ''}`}>
      <div className="rail-tabs">
        {RAIL_TABS.map((t) => {
          const meta = railTabMeta(session, t);
          return (
            <button
              key={t}
              className={`rail-tab${tab === t ? ' active' : ''}${meta.alert ? ' alert' : ''}`}
              onClick={() => onTab(t)}
            >
              {meta.label}
              {meta.badge && <span className="rail-badge">{meta.badge}</span>}
            </button>
          );
        })}
        <button className="rail-tab rail-close" title="Close the panel (x)" onClick={onClose}>
          <IconX size={13} />
        </button>
      </div>
      <div className="rail-content">
        {tab === 'findings' &&
          (session.findings.length > 0 ? (
            <FindingsPanel
              session={session}
              findingStates={findingStates}
              onSetFindingState={onSetFindingState}
              onJumpToFile={onJumpToFile}
            />
          ) : (
            <div className="faint" style={{ fontSize: 'var(--fs-sm)' }}>
              No findings for this session.
            </div>
          ))}
        {tab === 'session' && (
          <SessionPanel
            session={session}
            rejectedCategories={rejectedCategories}
            onRejectCategory={onRejectCategory}
            onClearCategory={onClearCategory}
          />
        )}
        {tab === 'timeline' && <TimelinePanel session={session} />}
        {tab === 'git' && <GitTab session={session} approvedPaths={approvedPaths} onChanged={onGitChanged} />}
      </div>
    </aside>
  );
}
