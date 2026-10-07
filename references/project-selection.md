# Host and project selection

Read for project-scoped work. Resolve once and reuse the user's selection and persistence decision within their authorized scope.

## Configuration

Host precedence: `--host`, `OPENPROJECT_URL`, project configuration, global configuration. Global `openproject/config.json` lives under XDG config on Linux, Application Support on macOS, or AppData on Windows; it accepts a non-secret `host` only.

Repository `.openproject.json` lives at the Git root (current directory outside Git) and accepts `host` plus `project_id` or the compatibility key `project`:

```json
{"host":"https://openproject.example.com","project_id":13}
```

A valid `project_id` is the default binding for relevant repository queries. Verify it with `openproject project --cwd . --json`, or pass the known root as `--cwd`. An explicit `--project` overrides that command's default; honor project IDs supplied by the user or relevant repository guidance.

## Missing binding: selection and persistence decision

Without a valid repository `project_id`, resolve the project and obtain the user's selection and persistence decision **before the requested project-scoped command, including reads**. Authentication, directory matching, and a CLI default do not authorize using or persisting a binding. `project --cwd . --json` and `projects --json` may be used for resolution.

The CLI attempts exact normalized matches against project names/identifiers using the project directory name, current directory name, first README heading, and common manifest names (`Cargo.toml`, `pyproject.toml`, `package.json`, `composer.json`). Accept an exact result only when matching evidence identifies one project. Related-project suggestions are candidates to show the user; do not select them speculatively or search arbitrary README text for a match.

- **One exact match:** show its name and ID, offering **Create/update `.openproject.json` with this ID**, **Use it only for this request**, and **Use a different project ID** (free text).
- **Missing or ambiguous match:** show viable candidates, **Use a different project ID**, and **Do not select a project**. After selection, offer **Persist in `.openproject.json`** or **Use only for this request**.
- **Already selected or authorized:** reuse that decision; do not ask again for a project or persistence choice already provided. Explicit permission to persist allows creating/updating the binding while preserving `host` and other supported settings. A use-once choice means an explicit `--project ID` for this request and no file write.

`openproject project --project ID --bind --dry-run --json` previews the binding path and numeric ID; remove `--dry-run` only with persistence authorization. This is a local configuration write, separate from API writes. If the user declines project selection, stop the project-scoped work and report that unresolved decision.
