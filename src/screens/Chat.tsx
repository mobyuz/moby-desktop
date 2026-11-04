import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { MobyFace } from "../face/MobyFace";
import { exceedsLimit, MAX_TEXT_BYTES } from "../history";
import { Icon } from "../icons";
import { useApp } from "../store";

export function Chat() {
  const app = useApp();
  const [text, setText] = useState("");
  const listRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLTextAreaElement>(null);
  const empty = app.messages.length === 0;
  const waiting = app.sending && app.live === null;
  const tooLong = exceedsLimit(text);

  useEffect(() => {
    const list = listRef.current;
    if (list) list.scrollTop = list.scrollHeight;
  }, [app.messages.length, app.live, app.chatError, waiting]);

  useEffect(() => inputRef.current?.focus(), []);

  const submit = () => {
    if (app.sending || tooLong || text.trim() === "") return;
    app.send(text);
    setText("");
  };

  const onKeyDown = (event: KeyboardEvent<HTMLTextAreaElement>) => {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      submit();
    }
  };

  return (
    <main className="chat" data-empty={empty}>
      <div className="hero">
        <MobyFace expression={app.expression} />
        {empty && <h1>How can I help?</h1>}
      </div>

      {!empty && (
        <div className="messages" ref={listRef} role="log" aria-live="polite">
          {app.messages.map((message) => (
            <p key={message.id} className={`bubble ${message.role}`}>
              {message.text}
            </p>
          ))}
          {app.live !== null && <p className="bubble moby">{app.live}</p>}
          {app.notes.map((note, index) => (
            <p key={index} className="tool-note">
              {note}
            </p>
          ))}
          {waiting && (
            <div className="bubble moby typing">
              <span className="sr">Moby is thinking</span>
              <i />
              <i />
              <i />
            </div>
          )}
          {app.chatError && (
            <div className="turn-error" role="alert">
              <span>{app.chatError}</span>
              <button className="secondary small" type="button" onClick={app.retryTurn}>
                Try again
              </button>
            </div>
          )}
        </div>
      )}

      {tooLong && (
        <p className="error composer-note" role="alert">
          That message is too long. Keep it under {MAX_TEXT_BYTES / 1024} KB.
        </p>
      )}
      <form
        className="composer"
        onSubmit={(event) => {
          event.preventDefault();
          submit();
        }}
      >
        <textarea ref={inputRef} rows={1} aria-label="Message to Moby" placeholder="Message Moby" value={text} onChange={(e) => setText(e.target.value)} onKeyDown={onKeyDown} />
        {app.sending ? (
          <button className="secondary" type="button" onClick={app.stop}>
            Stop
          </button>
        ) : (
          <button className="send" type="submit" aria-label="Send" title="Send" disabled={tooLong || text.trim() === ""}>
            <Icon name="send" />
          </button>
        )}
      </form>
    </main>
  );
}
