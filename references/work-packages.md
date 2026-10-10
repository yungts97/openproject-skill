# Work-package operations

Read for requested changes, time logging, relations, attachments, or their detailed inspection. Use command help for flags supported by the installed CLI. Resolve the project through [project selection](project-selection.md) when the operation needs one.

## Start and finish

**Start work:** fetch `task ID --full --json`, determine the requested assignee and exact target status (consult `statuses --json` if needed), and update only authorized fields. Do not guess among plausible workflow states. For example, when self-assignment and that status are authorized:

```bash
openproject update 123 --assignee me --status "In progress" --dry-run --json
```

Remove `--dry-run` when the target/payload are settled and authorized, then fetch the task to verify assignee and status.

**Finish work:** identify the exact completion status from repository guidance and available statuses; ask only if it remains ambiguous. Comments, status/progress updates, and time entries are separate writes, authorized only to the extent the request specifies. Never invent hours or a time-entry activity. Fetch the work package immediately before the final update and verify the result. For a requested comment/description referring to a Git commit, use `openproject commit-link HEAD --format url` (or the requested commit) for a safe clickable URL when its remote is resolvable.

## Delete a work package

`delete TASK_ID` permanently deletes the work package and associated time entries. The server also deletes its entire child hierarchy. Use only for an explicitly requested deletion of an exact work-package ID; authorization to update or finish work does not authorize deletion.

```bash
openproject delete 123 --dry-run --json
openproject delete 123 --cascade --dry-run --json
```

The command fetches the task first. OpenProject omits `_links.children` when there are no visible children; the CLI treats that as an empty list. It refuses deletion if `_links` is missing or invalid, or if a present `children` value is not an array. It requires `--cascade` when the API reports child links, even in a dry run. The preview includes the target summary, visible direct child links, cascade flag, and a permanent-deletion warning; it performs reads but never sends DELETE. Remove `--dry-run` once the target and scope are authorized. Use `--cascade` only when deletion of the entire child hierarchy and its time entries is authorized; never add it automatically to bypass an error.

The check reflects API visibility at read time. Hidden children and hierarchy changes between GET and DELETE cannot be ruled out, and `--cascade` covers all descendants, not just previewed links. The server enforces deletion permissions. Success returns `deleted: true`, `taskId`, and `cascade`. Verify with `task ID --full --json`: expect HTTP 404, which means absent or no longer visible. After an uncertain DELETE outcome, inspect the state before retrying and report uncertainty if absence cannot be established.

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

## Comments

Use `comments TASK_ID` for focused discussion history. It includes nonempty comments attached to change activities as well as standalone comments, in API activity order. `--author` takes a numeric user ID or `me`; `--since` takes a positive number of preceding 24-hour days (`7` or `7d`) and filters creation time, not edit time.

```bash
openproject comments 123 --author me --since 7d --limit 25 --json
openproject comment 123 --message "Implemented the API change." --dry-run --json
openproject comment 123 --message-file ./update.md --dry-run --json
openproject comment 123 --message-file - --dry-run --json < ./update.md
openproject comment edit 456 --message "Corrected implementation details." --dry-run --json
openproject comment 123 --quote 456 --message "Agreed; the follow-up is ready." --dry-run --json
openproject comment attach 456 ./screenshot.png --description "Reproduction evidence" --dry-run --json
openproject comment react 456 --emoji '👍' --dry-run --json
openproject comment reactions 456 --json
```

Comment listing scans and deduplicates activity pages so filtering does not lose matches. Pagination applies to matching comments: use the returned `nextOffset` with the same `--limit` and filters until it is null. `total` is null when more matches exist; it is exact only after the end of history is reached. A filtered empty page at the end is a complete result, but a read failure or broken pagination must not be treated as proof that no comments exist. History changes between invocations can shift pages.

Adding and editing each require exactly one of `--message` or `--message-file`. File/stdin input must be UTF-8; `--message-file -` requires piped or redirected stdin. Preserve user-provided Markdown and whitespace; empty or whitespace-only input is refused.

Editing is an external write that requires an explicit request to replace the text of that exact comment. Use the **activity ID**, not its work-package ID. The CLI reads the activity first, requires a nonempty comment and an update link, then PATCHes only the text. It preserves internal visibility and change details. Edit dry runs perform the read but never PATCH; add dry runs never POST. After a requested edit, verify with `activity ACTIVITY_ID --json`. As with new comments, do not blindly retry an edit after an uncertain write outcome; inspect the current text first.

Use `comment TASK_ID --quote ACTIVITY_ID` with a message or message file to post a new comment containing a linked source reference, Markdown blockquote, and reply. The source must belong to the same work package and have nonempty comment text. This copies the source text without creating a threaded reply or automatically mentioning its author. Internal source comments always produce internal replies; do not strip internal visibility or retry as public if the server denies permission. Quote dry runs read the source but never POST. Verify the newly created activity and its visibility after a requested quote.

Use `comment attach ACTIVITY_ID FILE` for an explicitly requested upload to that exact comment. A local regular file and an existing nonempty comment with an `addAttachment` link are required. The basename and detected MIME type are defaults; optional `--name`, `--description`, and `--content-type` customize the metadata. Dry runs validate the file, read the comment, and preview the upload without sending file contents. Upload permissions and internal-comment attachment visibility are enforced by the server. Verify the returned attachment via `attachment show ATTACHMENT_ID --json`, checking its container, and inspect the target activity. Posting a comment and attaching a file are separate writes; report each result separately when both were requested.

Use `comment react ACTIVITY_ID --emoji REACTION` only for an explicitly requested reaction on that exact comment. It toggles the current user's reaction: repeating the same emoji removes it. Accept an API identifier or its corresponding emoji: `thumbs_up` (👍), `thumbs_down` (👎), `grinning_face_with_smiling_eyes` (😄), `confused_face` (😕), `heart` (❤️ or ❤), `party_popper` (🎉), `rocket` (🚀), or `eyes` (👀). The CLI reads the activity and requires nonempty comment text before PATCHing only the reaction identifier. A dry run reads the comment and previews the toggle without writing. Server permissions govern reactions on public and internal comments; never change visibility to bypass a denial. Reaction support requires a server with the activity emoji-reaction API.

`comment reactions ACTIVITY_ID --json` returns the complete reaction collection, with counts and `reactingUsers` links. Verify a requested toggle by checking whether the current user's link is present for the requested reaction. A toggle is a separate write from posting, editing, quoting, or attaching files. After an uncertain write outcome, inspect the current user's membership before retrying: repeating a successful toggle would undo it. The CLI does not automatically retry reaction PATCH requests.

## Relations

`relations ID` returns relations where the task is either endpoint, plus separate `hierarchy` parent/child links. Interpret `_links.from`, `_links.to`, and `type`: `blocks` means from blocks to; `blocked` means from is blocked by to. Verify the blocking task's state before declaring an unresolved blocker. Parent/child, duplicate, and `relates` links do not prove blocking; scheduling predecessors are dependencies.

```bash
openproject relation add 123 --to 456 --type blocks --dry-run --json
openproject relation delete 789 --dry-run --json
```

## Attachments

`attachments ID` lists attachments; `attachment show ATTACHMENT_ID` returns metadata. Upload a local regular file with `attachment upload`; basename and detected MIME type are automatic, with optional `--name` and `--content-type` overrides. Use `comment attach` to target an existing comment. Deletion is permanent and requires an explicit request.

```bash
openproject attachment upload 123 ./build.log --description "Build evidence" --dry-run --json
openproject attachment download 901 --output ./build.log --dry-run --json
openproject attachment delete 901 --dry-run --json
```

A download writes locally. Its default destination is the server filename in the current directory; an existing file is refused unless `--force` is explicit, and a symbolic-link destination is never followed. Review an uncertain destination with `--dry-run` before writing.
