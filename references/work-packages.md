# Work-package operations

Read for requested changes, time logging, relations, attachments, or their detailed inspection. Use command help for flags supported by the installed CLI. Resolve the project through [project selection](project-selection.md) when the operation needs one.

## Start and finish

**Start work:** fetch `task ID --full --json`, determine the requested assignee and exact target status (consult `statuses --json` if needed), and update only authorized fields. Do not guess among plausible workflow states. For example, when self-assignment and that status are authorized:

```bash
openproject update 123 --assignee me --status "In progress" --dry-run --json
```

Remove `--dry-run` when the target/payload are settled and authorized, then fetch the task to verify assignee and status.

**Finish work:** identify the exact completion status from repository guidance and available statuses; ask only if it remains ambiguous. Comments, status/progress updates, and time entries are separate writes, authorized only to the extent the request specifies. Never invent hours or a time-entry activity. Fetch the work package immediately before the final update and verify the result. For a requested comment/description referring to a Git commit, use `openproject commit-link HEAD --format url` (or the requested commit) for a safe clickable URL when its remote is resolvable.

## Fields and filtering

- Resolve project, user, type, status, priority, and version names exactly or use IDs. Relationship values in API payloads use `_links` with `href`.
- Clear mutable values deliberately with `update --clear-*`; never combine a value with its corresponding clear option.
- Custom-field keys/types must come from a trusted schema, API response, user input, or repository guidance. Use `--custom-field customFieldN=JSON` for scalars and `--custom-field-link customFieldN=/api/v3/RESOURCE/ID` for links; never guess the numeric key or type.
- `tasks` supports repeated status, type, priority, and sort flags. Values within a filter are alternatives; different filters are conjunctive. `--updated-since` takes a positive day count (`7` or `7d`).

Payload-review examples:

```bash
openproject create --project 13 --subject "Fix approval flow" --type Task --assignee me --priority High --dry-run --json
openproject update 123 --percent 40 --responsible me --clear-due-date --dry-run --json
```

## Time and activity

Before logging time, read `time-entry-activities ID --json` for activities allowed by the time-entry form. Use the user's specified activity if allowed; otherwise show the available names and ask them to choose. Do not invent duration, date, or activity.

```bash
openproject log-time 123 --hours 1.5 --date 2026-09-03 --comment "Implementation" --activity Development --dry-run --json
```

`activities ID` expands each entry to its full activity resource; `activity ACTIVITY_ID` fetches one directly. Read activities only when relevant.

## Relations

`relations ID` returns relations where the task is either endpoint, plus separate `hierarchy` parent/child links. Interpret `_links.from`, `_links.to`, and `type`: `blocks` means from blocks to; `blocked` means from is blocked by to. Verify the blocking task's state before declaring an unresolved blocker. Parent/child, duplicate, and `relates` links do not prove blocking; scheduling predecessors are dependencies.

```bash
openproject relation add 123 --to 456 --type blocks --dry-run --json
openproject relation delete 789 --dry-run --json
```

## Attachments

`attachments ID` lists attachments; `attachment show ATTACHMENT_ID` returns metadata. Upload a local regular file with `attachment upload`; basename and detected MIME type are automatic, with no `--name` or `--content-type` overrides. Deletion is permanent and requires an explicit request.

```bash
openproject attachment upload 123 ./build.log --description "Build evidence" --dry-run --json
openproject attachment download 901 --output ./build.log --dry-run --json
openproject attachment delete 901 --dry-run --json
```

A download writes locally. Its default destination is the server filename in the current directory; an existing file is refused unless `--force` is explicit, and a symbolic-link destination is never followed. Review an uncertain destination with `--dry-run` before writing.
