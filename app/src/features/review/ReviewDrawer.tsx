import { NoteField } from '../../components/NoteField';
import { IconX } from '../../components/icons';
import { useDismiss } from '../../state/useDismiss';

// Your review for the agent, over the code: every decision as an
// instruction. ARS changes nothing itself, so a rejected change becomes an
// UNDO the agent carries out. Edit the text if you like, then copy it.
// Copying records a round: the next review shows only what changed since.

/** The usual rough measure for code and English */
const CHARS_PER_TOKEN = 4;
/** About what one row of the text box holds, to size it to the text */
const CHARS_PER_ROW = 90;
const MAX_ROWS = 20;

const rowsFor = (text: string) =>
  Math.min(MAX_ROWS, text.split('\n').reduce((n, line) => n + Math.max(1, Math.ceil(line.length / CHARS_PER_ROW)), 1));

interface ReviewDrawerProps {
  agentLabel: string;
  projectName: string;
  /** What will be copied: your edited text, or the review as ARS writes it */
  text: string;
  /** The review as ARS writes it */
  generated: string;
  items: number;
  busy: boolean;
  /** Your edited text; '' goes back to the review as ARS writes it */
  onEdit: (text: string) => void;
  onCopy: () => void;
  onClose: () => void;
}

export function ReviewDrawer({ agentLabel, projectName, text, generated, items, busy, onEdit, onCopy, onClose }: ReviewDrawerProps) {
  useDismiss(true, onClose);
  const edited = text !== generated;
  const tokens = Math.max(1, Math.round(text.length / CHARS_PER_TOKEN));
  const saveMarkdown = () => {
    const url = URL.createObjectURL(new Blob([text], { type: 'text/markdown' }));
    const a = document.createElement('a');
    a.href = url;
    a.download = `review-${projectName.replace(/[^A-Za-z0-9._-]/g, '-')}.md`;
    a.click();
    URL.revokeObjectURL(url);
  };
  return (
    <div className="review-drawer" role="region" aria-label="Your review for the agent">
      <div className="spread">
        <strong className="review-drawer-title">
          Your review for {agentLabel} · {items} item{items === 1 ? '' : 's'}
          <span className="faint" title="Rough size in the agent's context">
            {' '}
            · ~{tokens} tokens
          </span>
        </strong>
        <button className="icon-btn" title="Close (Esc)" aria-label="Close" onClick={onClose}>
          <IconX size={14} />
        </button>
      </div>
      <NoteField
        className="textarea review-text mt-2"
        rows={rowsFor(text)}
        aria-label="Your review for the agent"
        value={text}
        onSave={(t) => onEdit(t === generated ? '' : t)}
      />
      <div className="review-drawer-actions">
        <button
          className="btn btn-primary btn-tool-primary"
          disabled={busy}
          title="Copy it to paste into the agent. Next time you see only what changes from here."
          onClick={onCopy}
        >
          Copy review
        </button>
        <button className="btn-tool" title="Save the review as a Markdown file" onClick={saveMarkdown}>
          Save .md
        </button>
        {edited && (
          <button className="btn-mini" title="Go back to the review as ARS writes it" onClick={() => onEdit('')}>
            Discard my edits
          </button>
        )}
      </div>
      <p className="faint review-tip">
        Paste it into {agentLabel}, or ask {agentLabel} to run <span className="mono">ars feedback</span> in the project.
      </p>
    </div>
  );
}
