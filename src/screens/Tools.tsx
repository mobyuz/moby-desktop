import { useState, type FormEvent } from "react";
import { useApp } from "../store";
import type { ToolName } from "../types";

const TOOL_INFO: Record<ToolName, { label: string; detail: string }> = {
  git_status: { label: "Git status", detail: "Read-only. Only registered projects." },
  open_app: { label: "Open an app", detail: "Only apps on your allow-list." },
  open_project: { label: "Open a project", detail: "Opens a registered project in your editor." },
  search_files: { label: "Search files", detail: "Read-only, inside approved folders, bounded query." },
  create_reminder: { label: "Create a reminder", detail: "Saves a short local reminder after you approve it." },
};

const APPS = ["Visual Studio Code", "Windows Terminal", "File Explorer"];

export function Tools() {
  const app = useApp();
  const [name, setName] = useState("");
  const [path, setPath] = useState("");

  const add = (event: FormEvent) => {
    event.preventDefault();
    if (name.trim() === "" || path.trim() === "") return;
    app.addProject(name.trim(), path.trim());
    setName("");
    setPath("");
  };

  return (
    <div className="stack">
      <section className="card">
        <h2>Tools Moby may ask to use</h2>
        <p className="hint">Moby can only ask. Nothing runs until you approve it, and turned-off tools are refused automatically.</p>
        <ul className="list">
          {(Object.keys(TOOL_INFO) as ToolName[]).map((tool) => (
            <li key={tool} className="row between">
              <span>
                <strong>{TOOL_INFO[tool].label}</strong>
                <span className="muted block">{TOOL_INFO[tool].detail}</span>
              </span>
              <label className="switch">
                <input type="checkbox" role="switch" checked={app.tools[tool]} onChange={() => app.toggleTool(tool)} aria-label={`${TOOL_INFO[tool].label} enabled`} />
                <span>{app.tools[tool] ? "On" : "Off"}</span>
              </label>
            </li>
          ))}
        </ul>
      </section>

      <section className="card">
        <h2>Registered projects</h2>
        <ul className="list">
          {app.projects.map((project) => (
            <li key={project.id} className="row between">
              <span>
                <strong>{project.name}</strong>
                <span className="muted block">{project.path}</span>
              </span>
              <button className="secondary small" type="button" onClick={() => app.removeProject(project.id)}>
                Remove
              </button>
            </li>
          ))}
          {app.projects.length === 0 && <li className="muted">No projects registered.</li>}
        </ul>
        <form className="row" onSubmit={add}>
          <label className="grow">
            Name
            <input value={name} onChange={(e) => setName(e.target.value)} />
          </label>
          <label className="grow">
            Folder
            <input value={path} onChange={(e) => setPath(e.target.value)} placeholder="D:\repos\my-project" />
          </label>
          <button className="primary" type="submit">
            Add
          </button>
        </form>
      </section>

      <section className="card">
        <h2>Allowed apps</h2>
        <ul className="chips-static">
          {APPS.map((item) => (
            <li key={item}>{item}</li>
          ))}
        </ul>
      </section>
    </div>
  );
}
