import { useCallback, useEffect, useState } from "react";
import { api, asFailure } from "../api";
import { formatWhen } from "../history";
import { accessFor, READ_TOOLS, reminderState, sessionGrantLabel } from "../tools";
import type { Registry, Reminder } from "../types";

const EMPTY: Registry = { projects: [], apps: [], grants: [], session_grants: [] };

/** Folders and programs Moby may use, what it may do with them, and its reminders. */
export function Tools() {
  const [registry, setRegistry] = useState<Registry>(EMPTY);
  const [reminders, setReminders] = useState<Reminder[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [removing, setRemoving] = useState<string | null>(null);

  const reload = useCallback(async () => {
    const [next, list] = await Promise.all([api.listRegistry(), api.listReminders()]);
    setRegistry(next);
    setReminders(list);
  }, []);

  useEffect(() => {
    reload().catch((failure: unknown) => setError(asFailure(failure).message));
  }, [reload]);

  const attempt = (action: Promise<unknown>, done?: string) => {
    setError(null);
    setNotice(null);
    action
      .then(reload)
      .then(() => done && setNotice(done))
      .catch((failure: unknown) => setError(asFailure(failure).message));
  };

  const add = (pick: () => Promise<unknown>) => {
    setError(null);
    setNotice(null);
    pick()
      .then(async (added) => {
        await reload();
        if (added) setNotice("Added.");
      })
      .catch((failure: unknown) => setError(asFailure(failure).message));
  };

  return (
    <main className="page">
      <div className="page-column">
        <section>
          <h2>Tools and projects</h2>
          <p>
            Moby can only use the folders and programs you add here. It sees their names, never their locations, and it can never run anything you did not add. Opening a program or a project always asks you first,
            every time, in a separate Windows window that Moby&rsquo;s page cannot click for you.
          </p>
          {notice && (
            <p className="ok" role="status">
              {notice}
            </p>
          )}
          {error && (
            <p className="error" role="alert">
              {error}
            </p>
          )}
        </section>

        <section>
          <div className="row between">
            <h2>Projects</h2>
            <button className="primary small" type="button" onClick={() => add(api.addProject)}>
              Add project folder
            </button>
          </div>
          <p className="notice">
            Adding a folder lets Moby read its <strong>file names</strong> and <strong>Git status</strong>, without asking each time. It cannot read what is inside your files. You can turn either off below, or remove the
            folder, whenever you like.
          </p>
          {registry.projects.length === 0 ? (
            <p className="muted">No projects yet. Adding a folder lets Moby read its Git status and search its file names.</p>
          ) : (
            <ul className="list">
              {registry.projects.map((project) => (
                <li key={project.id}>
                  <div className="row between">
                    <div className="grow">
                      <strong>{project.name}</strong>
                      <div className="hint path">{project.short_path}</div>
                    </div>
                    {removing === project.id ? (
                      <div className="row">
                        <button
                          className="danger small"
                          type="button"
                          onClick={() => {
                            setRemoving(null);
                            attempt(api.removeProject(project.id), "Removed, with its permissions.");
                          }}
                        >
                          Remove
                        </button>
                        <button className="secondary small" type="button" onClick={() => setRemoving(null)}>
                          Cancel
                        </button>
                      </div>
                    ) : (
                      <button className="secondary small" type="button" aria-label={`Remove ${project.name}`} onClick={() => setRemoving(project.id)}>
                        Remove
                      </button>
                    )}
                  </div>
                  <ul className="grants">
                    {READ_TOOLS.map(({ tool, label }) => {
                      const access = accessFor(registry, tool, project.id);
                      return (
                        <li key={tool} className="row between">
                          <span>
                            {label}
                            <span className="tag">{access.kind === "always" ? "Allowed" : access.kind === "session" ? "Allowed this session" : "Not allowed"}</span>
                          </span>
                          <span className="row">
                            {access.kind === "always" && (
                              <button className="secondary small" type="button" aria-label={`Revoke ${label} for ${project.name}`} onClick={() => attempt(api.revokeGrant(access.grant.id), "Permission revoked.")}>
                                Revoke
                              </button>
                            )}
                            {access.kind === "session" && (
                              <button className="secondary small" type="button" aria-label={`Revoke ${label} for ${project.name}`} onClick={() => attempt(api.revokeSessionGrant(tool, access.grant.key), "Permission revoked.")}>
                                Revoke
                              </button>
                            )}
                            {access.kind !== "always" && (
                              <button className="secondary small" type="button" aria-label={`Always allow ${label} for ${project.name}`} onClick={() => attempt(api.grantRead(project.id, tool, false))}>
                                Allow always
                              </button>
                            )}
                            {access.kind === "none" && (
                              <button className="secondary small" type="button" aria-label={`Allow ${label} for ${project.name} this session`} onClick={() => attempt(api.grantRead(project.id, tool, true))}>
                                Allow this session
                              </button>
                            )}
                          </span>
                        </li>
                      );
                    })}
                  </ul>
                </li>
              ))}
            </ul>
          )}
        </section>

        <section>
          <div className="row between">
            <h2>Programs</h2>
            <button className="primary small" type="button" onClick={() => add(api.addApp)}>
              Add program
            </button>
          </div>
          {registry.apps.length === 0 ? (
            <p className="muted">No programs yet. Only .exe programs can be added, and Moby asks every time before opening one.</p>
          ) : (
            <ul className="list">
              {registry.apps.map((program) => (
                <li key={program.id} className="row between">
                  <div className="grow">
                    <strong>{program.name}</strong>
                    <div className="hint path">{program.short_path}</div>
                  </div>
                  <button className="secondary small" type="button" aria-label={`Remove ${program.name}`} onClick={() => attempt(api.removeApp(program.id), "Removed.")}>
                    Remove
                  </button>
                </li>
              ))}
            </ul>
          )}
          {registry.session_grants.some((grant) => grant.tool === "open_app" || grant.tool === "open_project") && (
            <div>
              <h3>Allowed until you close Moby</h3>
              <ul className="list">
                {registry.session_grants
                  .filter((grant) => grant.tool === "open_app" || grant.tool === "open_project")
                  .map((grant) => (
                    <li key={`${grant.tool}:${grant.key}`} className="row between">
                      <span>{sessionGrantLabel(registry, grant)}</span>
                      <button className="secondary small" type="button" onClick={() => attempt(api.revokeSessionGrant(grant.tool, grant.key), "Permission revoked.")}>
                        Revoke
                      </button>
                    </li>
                  ))}
              </ul>
            </div>
          )}
        </section>

        <section>
          <h2>Reminders</h2>
          {reminders.length === 0 ? (
            <p className="muted">No reminders. Ask Moby to remind you of something and confirm it.</p>
          ) : (
            <ul className="list">
              {reminders.map((reminder) => (
                <li key={reminder.id} className="row between">
                  <div className="grow">
                    <span>{reminder.text}</span>
                    <div className="hint">
                      {formatWhen(reminder.due_at)}
                      <span className="tag" data-state={reminder.state}>
                        {reminderState(reminder.state)}
                      </span>
                    </div>
                  </div>
                  <button className="secondary small" type="button" aria-label={`Delete reminder: ${reminder.text}`} onClick={() => attempt(api.deleteReminder(reminder.id), "Reminder deleted.")}>
                    Delete
                  </button>
                </li>
              ))}
            </ul>
          )}
        </section>
      </div>
    </main>
  );
}
