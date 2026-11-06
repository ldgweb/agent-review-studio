import { useState } from 'react';
import { IconCheck, IconX } from '../../components/icons';
import { copyText } from '../../lib/clipboard';
import { platform } from '../../lib/desktop';
import { approvalText, commitCommands } from '../../lib/review';
import { useDismiss } from '../../state/useDismiss';

// The end of a review: every change is approved and there is nothing for
// the agent to change. What is left is the commit, which ARS never makes:
// either the user commits (the commands are ready to copy), or the agent
// does it on request. Once the commit lands, the page turns to "Everything
// is committed" by itself.

interface FinishDrawerProps {
  agentLabel: string;
  projectName: string;
  root: string;
  busy: boolean;
  /** Copy the approval for the agent and record it as a round */
  onAskAgent: (text: string) => void;
  onClose: () => void;
}

const shell = platform === 'windows' ? 'powershell' : 'posix';

export function FinishDrawer({ agentLabel, projectName, root, busy, onAskAgent, onClose }: FinishDrawerProps) {
  useDismiss(true, onClose);
  const [message, setMessage] = useState('');
  const [copied, setCopied] = useState<'' | 'ok' | 'failed'>('');
  const ready = message.trim().length > 0;
  // Until there is a message, the command shows where it goes
  const commands = commitCommands(root, ready ? message : '…', shell);
  const copyCommands = () => void copyText(commands).then((ok) => setCopied(ok ? 'ok' : 'failed'));
  return (
    <div className="review-drawer finish-drawer" role="region" aria-label="Finish the review">
      <div className="spread">
        <strong className="finish-title">
          <span className="finish-mark" aria-hidden="true">
            <IconCheck size={14} />
          </span>
          Everything approved
        </strong>
        <button className="icon-btn" title="Close (Esc)" aria-label="Close" onClick={onClose}>
          <IconX size={14} />
        </button>
      </div>
      <p className="finish-lead">
        Nothing for {agentLabel} to change. To finish, commit the changes: ARS never commits for you.
      </p>

      <label className="why-label" htmlFor="finish-message">
        Commit message
      </label>
      <input
        id="finish-message"
        className="settings-input finish-message"
        autoFocus
        placeholder="What these changes do"
        value={message}
        onChange={(e) => {
          setMessage(e.target.value);
          setCopied('');
        }}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && ready) copyCommands();
        }}
      />
      <pre className="why-code finish-commands">{commands}</pre>
      <div className="review-drawer-actions">
        <button className="btn btn-primary btn-tool-primary" disabled={!ready} onClick={copyCommands}>
          {copied === 'ok' ? '✓ Copied' : 'Copy commands'}
        </button>
        <span className="faint finish-hint">
          {copied === 'failed'
            ? 'They could not be copied: select them above.'
            : `Paste them into a terminal${shell === 'powershell' ? ' (PowerShell)' : ''}, or commit from your editor.`}
        </span>
      </div>

      <div className="finish-or">
        <span>Or let {agentLabel} commit them</span>
        <button
          className="btn-tool"
          disabled={busy}
          title={`Copy a short note for ${agentLabel}: everything approved, commit it`}
          onClick={() => onAskAgent(approvalText(projectName))}
        >
          Copy for {agentLabel}
        </button>
      </div>
      <p className="faint review-tip">When the commit lands, this page shows “Everything is committed” by itself.</p>
    </div>
  );
}
