import { useApp } from "../store";
import type { ApprovalStatus } from "../types";

const LABEL: Record<ApprovalStatus, string> = {
  pending: "Waiting for you",
  approved: "Approved",
  denied: "Denied",
  blocked: "Blocked (tool off)",
};

export function Approvals() {
  const app = useApp();
  const pending = app.approvals.filter((item) => item.status === "pending");
  const past = app.approvals.filter((item) => item.status !== "pending");

  return (
    <div className="stack">
      <section className="card">
        <h2>Waiting for you</h2>
        {pending.length === 0 && <p className="muted">Nothing to approve. Ask Moby for the git status on the Home screen to see a request here.</p>}
        <ul className="list">
          {pending.map((item) => (
            <li key={item.id}>
              <strong>{item.title}</strong>
              <p className="muted">{item.detail}</p>
              <div className="row">
                <button className="primary small" type="button" onClick={() => app.resolveApproval(item.id, true)}>
                  Approve
                </button>
                <button className="secondary small" type="button" onClick={() => app.resolveApproval(item.id, false)}>
                  Deny
                </button>
              </div>
            </li>
          ))}
        </ul>
      </section>

      <section className="card">
        <h2>History</h2>
        {past.length === 0 && <p className="muted">No decisions yet.</p>}
        <ul className="list">
          {past.map((item) => (
            <li key={item.id} className="row between">
              <span>
                <strong>{item.title}</strong>
                <span className="muted block">{new Date(item.at).toLocaleTimeString("en", { timeStyle: "short" })}</span>
              </span>
              <span className={`tag ${item.status === "approved" ? "good" : "bad"}`}>{LABEL[item.status]}</span>
            </li>
          ))}
        </ul>
      </section>
    </div>
  );
}
