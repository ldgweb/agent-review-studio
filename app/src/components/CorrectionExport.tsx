import { useState } from 'react';
import type { AgentSession } from '../types/domain';

// Exports the assembled correction request for the agent that produced the
// session: copy it to the clipboard or save it as Markdown, then paste it
// into the agent's next prompt.

export function CorrectionExport({ session, prompt }: { session: AgentSession; prompt: string }) {
  const [copied, setCopied] = useState(false);

  const exportText = `# Correction request for ${session.agentLabel}\n# Project: ${session.projectName}${session.branch ? ` @ ${session.branch}` : ''}\n\n${prompt}\n`;
  const copy = () => {
    void navigator.clipboard
      ?.writeText(exportText)
      .then(() => {
        setCopied(true);
        setTimeout(() => setCopied(false), 2000);
      })
      .catch(() => undefined);
  };
  const download = () => {
    const blob = new Blob([exportText], { type: 'text/markdown' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = `corrections-${session.sessionId.slice(0, 8).replace(/[^A-Za-z0-9._-]/g, '-')}.md`;
    a.click();
    URL.revokeObjectURL(url);
  };

  return (
    <div className="mt-2">
      <div className="flex" style={{ gap: 'var(--sp-3)', alignItems: 'center', flexWrap: 'wrap' }}>
        <button className="btn btn-sm btn-primary" onClick={copy} title="Copy the correction request to the clipboard">
          {copied ? '✓ Copied' : `Copy for ${session.agentLabel}`}
        </button>
        <button className="btn btn-sm" onClick={download} title="Save the correction request as a Markdown file">
          Save .md
        </button>
        <span className="faint" style={{ fontSize: 'var(--fs-xs)' }}>
          Paste these corrections into your next {session.agentLabel} prompt.
        </span>
      </div>
    </div>
  );
}
