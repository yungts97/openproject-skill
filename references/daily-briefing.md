# Daily work briefing

Read for daily briefings, what to work on today in OpenProject, or overdue tasks and blockers. This workflow is read-only and uses the existing CLI.

Read [project selection](project-selection.md) before project-scoped collection; reuse an existing selection/persistence decision.

## Scope and collection

1. Resolve the project through the selection/persistence decision in [project selection](project-selection.md). Default to that project and `--assignee me`; honor an explicitly requested project, assignee, or team scope. Do not silently expand to other projects or all assignees.
2. Establish today's calendar date in the user's timezone from the session context. If it is unavailable, use the environment's local date and timezone and state that assumption. Default the upcoming window to tomorrow through today plus seven calendar days, inclusive; honor a requested window. OpenProject due dates are calendar dates, not UTC timestamps.
3. Collect open assigned tasks, including those without due dates:

   ```bash
   openproject tasks --project PROJECT_ID --assignee me --sort due-date:asc --sort priority:desc --limit 50 --offset 1 --json
   ```

   Replace `PROJECT_ID` with the resolved ID. For another assignee, resolve their exact identity with `users --project PROJECT_ID --json` and pass the numeric ID; omit `--assignee` only for an explicitly requested whole-project/team scope. `tasks` defaults to open statuses when neither `--all` nor `--status` is supplied. Do not use `--all`, a guessed status filter, `--due-before`, or `--updated-since` for this baseline: those can include closed work or omit undated and older active work.
4. Follow `next` with the same filters and limit, incrementing the one-based `--offset`, and deduplicate by task ID. Complete pagination before claiming totals or an empty workload. If a user-specified limit or a failed read stops collection, report the fetched count and remaining/unknown coverage and label the briefing partial.
5. Fetch `task TASK_ID --full --json` for details needed to rank or explain candidates. Compact task output omits priority and update timestamps; never invent those values. Use `statuses --limit 50 --json` and its remaining pages when needed to identify closed states from `isClosed` and to interpret the actual workflow. Use repository guidance or fetched status names to identify work in progress; do not assume every project calls it "In progress".

## Deadlines, blockers, and recommendations

- Classify open tasks as **overdue** when `dueDate` is before today, **due today** when equal, and **upcoming** when tomorrow through the window end. Keep undated tasks eligible for active-work recommendations. Exclude work found to be closed on a subsequent detail read.
- For today's recommendation candidates, inspect `relations TASK_ID --limit 50 --json` and its remaining pages. Interpret `_links.from`, `_links.to`, and `type`: `blocks` means from blocks to; `blocked` means from is blocked by to. Fetch the blocking task and check its status before describing an unresolved blocker. Parent/child, `relates`, and duplicate links do not themselves prove a blocker. Describe scheduling predecessors as dependencies rather than treating them as explicit blocking relations.
- Name and link each verified blocker, including its assignee when available. If a blocker is inaccessible or its state is unknown, say it could not be verified. Scope blocker conclusions to the tasks actually inspected; never turn failed or incomplete reads into "no blockers". Read recent `activities` only when needed to clarify a candidate, and attribute any blocker reported in a comment to that comment rather than presenting it as a verified relation.
- Recommend up to three concrete next actions using due-date urgency, verified priority, current work in progress, start dates, and dependencies. Prefer actionable work and avoid recommending starting tasks scheduled for a future date; for a blocked urgent task, recommend following up with the blocker owner rather than starting dependent work. Explain each choice briefly and flag overdue work scheduled to start later as a scheduling conflict. Treat this ranking as advice, not a change to OpenProject priority or status.

## Briefing output

State the project, assignee scope, local date/timezone, and upcoming window. Put recommended actions first, followed by deadlines, active work, and blockers in a concise layout suited to the request. Each task entry should use its returned `url` and include its ID, subject, status, due date when present, and a brief reason or next action. For tasks fetched only in full form, build the link from the resolved host and `/work_packages/TASK_ID`.

Include overdue, due-today, and upcoming counts only for completely fetched scopes; distinguish display truncation from collection truncation. State any failed reads, unverified statuses, or limited blocker checks. If the complete task collection is empty, say there are no open tasks assigned to the requested user in that project. Offer recommendations only; updates, comments, assignments, and time logging follow the existing external-write authorization rules.
