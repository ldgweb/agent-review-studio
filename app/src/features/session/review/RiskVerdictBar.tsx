import { useMemo } from 'react';
import type { AgentSession, RiskDimension } from '../../../types/domain';
import { RiskPill } from '../../../components/ui';
import { categoryFileIds } from '../../../lib/triage';

// Category verdicts: reject a whole risk dimension in one move — the
// articulated-verdict layer that a binary LGTM cannot express.

interface RiskVerdictBarProps {
  session: AgentSession;
  rejectedCategories: RiskDimension[];
  onRejectCategory: (dimension: RiskDimension, fileIds: string[]) => void;
  onClearCategory: (dimension: RiskDimension, fileIds: string[]) => void;
}

export function RiskVerdictBar({
  session,
  rejectedCategories,
  onRejectCategory,
  onClearCategory,
}: RiskVerdictBarProps) {
  const fileIdsByDimension = useMemo(() => {
    const map = new Map<RiskDimension, string[]>();
    for (const r of session.risks) map.set(r.dimension, categoryFileIds(session, r.dimension));
    return map;
  }, [session]);

  if (session.risks.length === 0) return null;

  return (
    <div className="card" style={{ padding: 'var(--sp-3) var(--sp-4)' }}>
      <div style={{ fontWeight: 600 }}>Category verdicts — reject a whole risk dimension</div>
      <div className="mt-3">
        {[...session.risks]
          .sort((a, b) => b.score - a.score)
          .map((r) => {
            const fileIds = fileIdsByDimension.get(r.dimension) ?? [];
            const rejected = rejectedCategories.includes(r.dimension);
            return (
              <div key={r.dimension} className="risk-verdict-row">
                <span title="Heuristic score (0–100) from deterministic checks, not a measured probability"><RiskPill level={r.level} label={`${r.dimension} ${r.score}`} /></span>
                <span className="verdict-rationale" title={r.rationale}>{r.rationale}</span>
                <span className="faint mono" style={{ flexShrink: 0, fontSize: 'var(--fs-xs)' }}>
                  {fileIds.length} file{fileIds.length !== 1 ? 's' : ''}
                </span>
                {rejected ? (
                  <button
                    className="btn btn-sm active"
                    title="Undo — resets the rejected files in this category to pending"
                    onClick={() => onClearCategory(r.dimension, fileIds)}
                  >
                    category rejected — undo
                  </button>
                ) : (
                  <button
                    className="btn btn-sm btn-danger"
                    disabled={fileIds.length === 0}
                    title={
                      fileIds.length === 0
                        ? 'No files carry this risk dimension'
                        : `Reject the ${fileIds.length} file${fileIds.length !== 1 ? 's' : ''} in this category`
                    }
                    onClick={() => onRejectCategory(r.dimension, fileIds)}
                  >
                    ✗ Reject category
                  </button>
                )}
              </div>
            );
          })}
      </div>
    </div>
  );
}
