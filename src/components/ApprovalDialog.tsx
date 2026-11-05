import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { useApp } from "../store";
import { approvalButtons, approvalText, ARM_DELAY_MS, SAFE_DECISION } from "../tools";

/**
 * The only place a local action is approved. It opens only for a dialog Rust issued, shows the
 * sentence the app wrote, keeps the model's words apart and marked untrusted, starts on Deny, and
 * never answers by itself: if nobody answers, Rust counts it as a denial.
 */
export function ApprovalDialog() {
  const app = useApp();
  const approval = app.approval;
  const denyRef = useRef<HTMLButtonElement>(null);
  const boxRef = useRef<HTMLDivElement>(null);
  const [armed, setArmed] = useState(false);
  const id = approval?.id;

  useEffect(() => {
    if (!id) return;
    const before = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    setArmed(false);
    denyRef.current?.focus();
    const timer = window.setTimeout(() => setArmed(true), ARM_DELAY_MS);
    return () => {
      window.clearTimeout(timer);
      before?.focus();
    };
  }, [id]);

  if (!approval) return null;
  const text = approvalText(approval);

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key === "Escape") {
      event.stopPropagation();
      void app.answerApproval("deny");
      return;
    }
    if (event.key !== "Tab") return;
    const buttons = Array.from(boxRef.current?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") ?? []);
    const first = buttons[0];
    const last = buttons[buttons.length - 1];
    if (!first || !last) return;
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  };

  return (
    <div className="scrim">
      <div ref={boxRef} className="dialog" role="alertdialog" aria-modal="true" aria-labelledby="approval-title" aria-describedby="approval-body" onKeyDown={onKeyDown}>
        <p className="eyebrow">Moby wants to do something on this computer</p>
        <h2 id="approval-title">{text.title}</h2>
        <div id="approval-body">
          {text.details.length > 0 && (
            <ul className="details">
              {text.details.map((line) => (
                <li key={line}>{line}</li>
              ))}
            </ul>
          )}
          {text.untrusted && (
            <div className="says">
              <strong>Moby says (not checked):</strong>
              <q>{text.untrusted}</q>
              <span className="hint">This is the model&rsquo;s own text. It does not change what will happen.</span>
            </div>
          )}
          <p className="hint">If you don&rsquo;t answer in {Math.round(approval.timeoutMs / 1000)} seconds, it counts as Deny.</p>
        </div>
        <div className="row dialog-actions">
          {approvalButtons(armed).map((button) => (
            <button
              key={button.decision}
              ref={button.decision === SAFE_DECISION ? denyRef : undefined}
              className={button.decision === SAFE_DECISION ? "primary" : "secondary"}
              type="button"
              disabled={button.disabled}
              onClick={() => void app.answerApproval(button.decision)}
            >
              {button.label}
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
