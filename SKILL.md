---
name: openproject
description: Manage OpenProject projects and work packages with the openproject CLI. Use for task queries, daily briefings, and requested work-package changes.
metadata:
  short-description: Manage OpenProject work packages
---

# OpenProject

Use the `openproject` CLI for OpenProject API v3 work. Prefer `--json`; inspect `openproject COMMAND --help` for flags rather than guessing. Only `auth login` is interactive.

## Load the relevant guidance

- **Project-scoped work:** read [project selection](references/project-selection.md) to resolve the host, repository binding, and any required selection/persistence decision before querying the project. Reuse the user's existing decision.
- **Daily briefing or what to work on today:** read [daily briefing](references/daily-briefing.md). This is a read-only workflow, not a CLI subcommand.
- **Changes to work packages, comments, time, relations, or attachments:** read [work-package operations](references/work-packages.md) for command-specific constraints, comment editing/quoting/uploads, deletion behavior, and start/finish workflows.
- **Missing CLI, failed authentication, installation, upgrade, or removal:** read [setup](references/setup.md).

Read only the references relevant to the request. Ordinary reads can use `projects`, `project`, `tasks`, `task`, `statuses`, `priorities`, `types`, `users`, `versions`, and `categories` directly once the project is resolved.

## Essential operating constraints

- Execute only the requested scope. `create`, `update`, `delete`, `comment`, `log-time`, `relation add/delete`, and `attachment upload/delete` are external writes and require an explicit user request for that action. Authentication and a briefing request do not authorize them. Work-package deletion permanently removes associated time entries and its child hierarchy; use `--cascade` only when that entire hierarchy is authorized. Honor authorization already given; use `--dry-run --json` when the target or payload still needs review rather than asking again for an approved action.
- Repository binding (`project --bind` or editing `.openproject.json`), attachment downloads, executable replacement, and uninstall are separate local writes. Binding requires a persistence decision; upgrade and uninstall require requests for those actions. Attachment deletion is permanent; `uninstall --purge` requires explicit complete-cleanup intent.
- Resolve names exactly or use numeric IDs. Read repository guidance relevant to the requested write. Fetch a work package immediately before updating it so its `lockVersion` is current, then verify the resulting state. Report separate writes separately if only part of the request succeeds.
- Comment quoting (`comment TASK_ID --quote ACTIVITY_ID`), file uploads (`comment attach ACTIVITY_ID FILE`), and emoji reaction toggles (`comment react ACTIVITY_ID --emoji REACTION`) are external writes covered by the same explicit-request rule. Quotes must remain in the source work package and preserve internal visibility. Posting a comment, attaching a file, and reacting are separate writes; verify and report each when requested. Repeating a reaction removes it; inspect `comment reactions ACTIVITY_ID --json` before retrying an uncertain toggle.
- Collections are paginated: use a bounded `--limit`, follow `next` using the same filters and one-based `--offset`, and deduplicate by ID. `comments` instead returns `nextOffset` for pages of matching comments; reuse the same filters and limit until it is null, and treat a null `total` as unknown. Finish collection before claiming exhaustive totals or an empty result; disclose incomplete reads. `tasks` defaults to open statuses unless `--all` or `--status` is supplied. `task --full` returns the API representation; compact output omits fields such as priority and update timestamps.
- Keep tokens and authorization headers out of chat, command arguments, and repository files. For missing credentials, direct the user to run `openproject auth login` in their own terminal and verify with `auth status --json` afterward.
- After an uncertain write outcome, inspect the server state before retrying. Do not blindly repeat creates, comments, time entries, or uploads; stop and report uncertainty if the outcome cannot be established. Runtime errors return a nonzero exit code and JSON stderr shaped as `{"error":{"message":"..."}}`.

Complete the authorized request through verification. Report relevant IDs, clickable task URLs, resulting changes, and any unresolved or partial results. A preview is sufficient only when the user requested a preview or a missing decision prevents execution.
