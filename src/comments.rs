use crate::{emit, id, resolve_user, write_exact, Cli, OpenProjectClient, PageArgs};
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Duration, Utc};
use clap::{Args, Subcommand};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::fs;
use std::io::{self, IsTerminal, Read};
use std::path::{Path, PathBuf};

#[derive(Args, Debug)]
#[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
pub(crate) struct CommentArgs {
    /// Work package to receive the new comment.
    #[arg(required = true, value_parser = clap::value_parser!(u64).range(1..))]
    task_id: Option<u64>,
    #[command(flatten)]
    message: MessageArgs,
    #[command(subcommand)]
    command: Option<CommentCommands>,
}

#[derive(Subcommand, Debug)]
enum CommentCommands {
    /// Replace the text of an existing comment (requires edit permission).
    Edit(EditArgs),
}

#[derive(Args, Debug)]
struct EditArgs {
    /// Activity ID, as shown by comments or activities.
    #[arg(value_parser = clap::value_parser!(u64).range(1..))]
    activity_id: u64,
    #[command(flatten)]
    message: MessageArgs,
}

#[derive(Args, Debug)]
#[group(required = true, multiple = false)]
struct MessageArgs {
    /// Comment text, in Markdown.
    #[arg(long)]
    message: Option<String>,
    /// UTF-8 Markdown file, or - to read redirected/piped stdin.
    #[arg(long, value_name = "PATH")]
    message_file: Option<PathBuf>,
}

impl MessageArgs {
    fn read(&self) -> Result<String> {
        if self.message_file.as_deref() == Some(Path::new("-")) && io::stdin().is_terminal() {
            bail!("--message-file - requires piped or redirected stdin");
        }
        self.read_from(&mut io::stdin().lock())
    }

    fn read_from(&self, input: &mut impl Read) -> Result<String> {
        let message = match (&self.message, &self.message_file) {
            (Some(message), None) => message.clone(),
            (None, Some(path)) if path == Path::new("-") => {
                let mut message = String::new();
                input
                    .read_to_string(&mut message)
                    .context("cannot read UTF-8 comment text from stdin")?;
                message
            }
            (None, Some(path)) => fs::read_to_string(path)
                .with_context(|| format!("cannot read UTF-8 comment file {}", path.display()))?,
            _ => bail!("provide exactly one of --message or --message-file"),
        };
        if message.trim().is_empty() {
            bail!("comment text must not be empty or whitespace-only");
        }
        Ok(message)
    }
}

#[derive(Args, Debug)]
pub(crate) struct CommentsArgs {
    #[arg(value_parser = clap::value_parser!(u64).range(1..))]
    task_id: u64,
    /// Filter by a numeric author ID or me.
    #[arg(long)]
    author: Option<String>,
    /// Comments created within this many days, e.g. 7 or 7d.
    #[arg(long)]
    since: Option<String>,
    #[command(flatten)]
    page: PageArgs,
}

pub(crate) fn execute(client: &OpenProjectClient, cli: &Cli, args: &CommentArgs) -> Result<Value> {
    match &args.command {
        Some(CommentCommands::Edit(args)) => {
            let message = args.message.read()?;
            let path = format!("/activities/{}", args.activity_id);
            let activity = client.get(&path)?;
            if activity
                .pointer("/comment/raw")
                .and_then(Value::as_str)
                .is_none_or(|text| text.trim().is_empty())
            {
                bail!("activity {} has no comment to edit", args.activity_id);
            }
            if activity
                .pointer("/_links/update/href")
                .and_then(Value::as_str)
                .filter(|href| !href.is_empty())
                .is_none()
            {
                bail!(
                    "comment {} cannot be edited with the current permissions",
                    args.activity_id
                );
            }
            // The activity version is read-only. Do not send work-package lockVersion,
            // internal visibility, or any other fields when replacing comment text.
            // Unlike comment creation, PATCH /activities/{id} takes a string.
            write_exact(
                client,
                cli,
                reqwest::Method::PATCH,
                &path,
                json!({"comment": message}),
            )
        }
        None => {
            let task_id = args.task_id.context("a work-package ID is required")?;
            let message = args.message.read()?;
            write_exact(
                client,
                cli,
                reqwest::Method::POST,
                &format!("/work_packages/{task_id}/activities"),
                json!({"comment": {"format": "markdown", "raw": message}}),
            )
        }
    }
}

fn since_cutoff(value: &str, now: DateTime<Utc>) -> Result<DateTime<Utc>> {
    let days: i64 = value
        .strip_suffix('d')
        .unwrap_or(value)
        .parse()
        .context("--since must be a positive day count, e.g. 7 or 7d")?;
    if days <= 0 {
        bail!("--since must be a positive day count, e.g. 7 or 7d");
    }
    Duration::try_days(days)
        .and_then(|duration| now.checked_sub_signed(duration))
        .context("--since day count is too large")
}

fn author_id(activity: &Value) -> Option<u64> {
    activity
        .pointer("/_links/user/href")
        .and_then(Value::as_str)
        .and_then(|href| href.trim_end_matches('/').rsplit('/').next()?.parse().ok())
        .or_else(|| {
            activity
                .pointer("/_embedded/user/id")
                .and_then(Value::as_u64)
        })
}

pub(crate) fn list(client: &OpenProjectClient, args: &CommentsArgs) -> Result<Value> {
    let cutoff = args
        .since
        .as_deref()
        .map(|value| since_cutoff(value, Utc::now()))
        .transpose()?;
    let author = args
        .author
        .as_deref()
        .map(|value| resolve_user(client, value))
        .transpose()?;
    if author == Some(0) {
        bail!("--author must be a positive user ID or 'me'");
    }
    let skip = u64::from(args.page.offset - 1) * u64::from(args.page.limit);
    let mut matched = 0_u64;
    let mut items = Vec::new();
    let mut next = format!(
        "/work_packages/{}/activities?pageSize=100&offset=1",
        args.task_id
    );
    let mut seen_pages = HashSet::new();
    let mut seen_activities = HashSet::new();
    let mut has_more = false;

    'pages: loop {
        if !seen_pages.insert(next.clone()) {
            bail!("OpenProject returned a repeated activity page; comment listing is incomplete");
        }
        let page = client.get(&next)?;
        let elements = page
            .pointer("/_embedded/elements")
            .and_then(Value::as_array)
            .context("OpenProject activity collection has no elements array")?;
        let mut new_entries = 0;
        for entry in elements {
            let activity_id = id(entry)?;
            if !seen_activities.insert(activity_id) {
                continue;
            }
            new_entries += 1;
            // Activity collections may abbreviate resources. Hydrate each entry so
            // comments attached to change activities are considered as well.
            let activity = client.get(&format!("/activities/{activity_id}"))?;
            let Some(text) = activity.pointer("/comment/raw").and_then(Value::as_str) else {
                continue;
            };
            if text.trim().is_empty() {
                continue;
            }
            if let Some(author) = author {
                let actual_author = author_id(&activity)
                    .context("OpenProject comment has no author ID; cannot apply --author")?;
                if actual_author != author {
                    continue;
                }
            }
            if let Some(cutoff) = cutoff {
                let created = activity.get("createdAt").and_then(Value::as_str).context(
                    "OpenProject comment has no creation timestamp; cannot apply --since",
                )?;
                if DateTime::parse_from_rfc3339(created)
                    .context("OpenProject comment has an invalid creation timestamp")?
                    < cutoff
                {
                    continue;
                }
            }
            matched += 1;
            if matched <= skip {
                continue;
            }
            // Read one additional matching comment to distinguish a full final
            // page from a page that has a continuation, without scanning all history.
            if items.len() == args.page.limit as usize {
                has_more = true;
                break 'pages;
            }
            items.push(json!({
                "id": activity_id,
                "taskId": args.task_id,
                "author": {
                    "id": author_id(&activity),
                    "name": activity.pointer("/_links/user/title")
                        .or_else(|| activity.pointer("/_embedded/user/name")),
                },
                "createdAt": activity.get("createdAt"),
                "updatedAt": activity.get("updatedAt"),
                "internal": activity.get("internal"),
                "comment": {"format": "markdown", "raw": text},
            }));
        }
        match page
            .pointer("/_links/nextByOffset/href")
            .and_then(Value::as_str)
        {
            Some(link) => {
                if new_entries == 0 {
                    bail!("OpenProject activity pagination made no progress; comment listing is incomplete");
                }
                next = link.to_owned();
            }
            None => {
                if page
                    .get("total")
                    .and_then(Value::as_u64)
                    .is_some_and(|total| total > seen_activities.len() as u64)
                {
                    bail!("OpenProject activity collection is missing its next-page link; comment listing is incomplete");
                }
                break;
            }
        }
    }
    let next_offset = if has_more {
        Some(
            args.page
                .offset
                .checked_add(1)
                .context("comment page offset is too large")?,
        )
    } else {
        None
    };
    Ok(json!({
        "items": items,
        "count": items.len(),
        "total": if has_more { None } else { Some(matched) },
        "offset": args.page.offset,
        "pageSize": args.page.limit,
        "nextOffset": next_offset,
    }))
}

pub(crate) fn emit_list(value: Value, as_json: bool) {
    if as_json {
        emit(value, true);
        return;
    }
    if let Some(items) = value.get("items").and_then(Value::as_array) {
        for item in items {
            let author = item
                .pointer("/author/name")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or_else(|| {
                    item.pointer("/author/id")
                        .and_then(Value::as_u64)
                        .map(|id| format!("user #{id}"))
                })
                .unwrap_or_else(|| "unknown author".to_owned());
            let created = item
                .get("createdAt")
                .and_then(Value::as_str)
                .unwrap_or("unknown date");
            let visibility = if item.get("internal").and_then(Value::as_bool) == Some(true) {
                " [internal]"
            } else {
                ""
            };
            println!("#{} {author} — {created}{visibility}", item["id"]);
            println!(
                "{}\n",
                item.pointer("/comment/raw")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            );
        }
    }
    if let Some(offset) = value.get("nextOffset").and_then(Value::as_u64) {
        println!(
            "More comments: repeat with the same filters and --limit, using --offset {offset}"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Commands;
    use clap::Parser;
    use std::io::Write;
    use std::net::TcpListener;
    use std::thread::JoinHandle;

    fn comment_args(cli: &Cli) -> &CommentArgs {
        match &cli.command {
            Commands::Comment(args) => args,
            _ => panic!("expected comment command"),
        }
    }

    fn list_args(cli: &Cli) -> &CommentsArgs {
        match &cli.command {
            Commands::Comments(args) => args,
            _ => panic!("expected comments command"),
        }
    }

    // Each response specifies the expected method/path. Capture JSON request
    // bodies as well so write tests verify the actual HTTP contract.
    fn mock_server(
        responses: Vec<(&'static str, u16, Value)>,
    ) -> (OpenProjectClient, JoinHandle<Vec<Value>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let mut bodies = Vec::new();
            for (expected, status, response) in responses {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buffer = [0_u8; 1024];
                let header_end = loop {
                    let read = stream.read(&mut buffer).unwrap();
                    assert!(read > 0, "request ended before headers");
                    request.extend_from_slice(&buffer[..read]);
                    if let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                        break end + 4;
                    }
                };
                let headers = String::from_utf8(request[..header_end].to_vec()).unwrap();
                assert_eq!(
                    headers.lines().next().unwrap(),
                    format!("{expected} HTTP/1.1")
                );
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                    .unwrap_or(0);
                while request.len() < header_end + length {
                    let read = stream.read(&mut buffer).unwrap();
                    assert!(read > 0, "request ended before body");
                    request.extend_from_slice(&buffer[..read]);
                }
                let request_body: Value = if length == 0 {
                    Value::Null
                } else {
                    serde_json::from_slice(&request[header_end..header_end + length]).unwrap()
                };
                // ActivitiesAPI declares the PATCH comment parameter as a String.
                // Enforce that contract rather than accepting any payload shape.
                let invalid_edit = expected.starts_with("PATCH /api/v3/activities/")
                    && !request_body.get("comment").is_some_and(Value::is_string);
                bodies.push(request_body);
                let (status, response) = if invalid_edit {
                    (400, json!({"message": "Bad request: comment is invalid"}))
                } else {
                    (status, response)
                };
                let body = response.to_string();
                write!(stream, "HTTP/1.1 {status} Response\r\nContent-Type: application/hal+json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
            bodies
        });
        let client =
            OpenProjectClient::new(format!("http://{address}"), "test-token".to_owned()).unwrap();
        (client, server)
    }

    fn activity(activity_id: u64, user_id: u64, text: &str) -> Value {
        json!({
            "id": activity_id,
            "_type": "Activity",
            "comment": {"raw": text},
            "createdAt": Utc::now().to_rfc3339(),
            "updatedAt": Utc::now().to_rfc3339(),
            "internal": true,
            "_links": {
                "user": {"href": format!("/api/v3/users/{user_id}"), "title": "Test author"},
                "update": {"href": format!("/api/v3/activities/{activity_id}"), "method": "patch"},
            },
        })
    }

    #[test]
    fn comment_cli_preserves_add_syntax_and_rejects_ambiguous_input() {
        for args in [
            vec!["openproject", "comment", "42", "--message", "hello"],
            vec![
                "openproject",
                "comment",
                "42",
                "--message-file",
                "update.md",
            ],
            vec![
                "openproject",
                "comment",
                "edit",
                "17",
                "--message",
                "corrected",
            ],
            vec![
                "openproject",
                "comment",
                "edit",
                "17",
                "--message-file",
                "-",
                "--dry-run",
                "--json",
            ],
        ] {
            let cli = Cli::try_parse_from(args).unwrap();
            let parsed = comment_args(&cli);
            assert!(
                parsed.task_id == Some(42)
                    || matches!(parsed.command, Some(CommentCommands::Edit(_)))
            );
        }
        for args in [
            vec!["openproject", "comment"],
            vec!["openproject", "comment", "42"],
            vec!["openproject", "comment", "edit", "17"],
            vec!["openproject", "comment", "--message", "hello"],
            vec![
                "openproject",
                "comment",
                "42",
                "--message",
                "hello",
                "--message-file",
                "-",
            ],
            vec![
                "openproject",
                "comment",
                "edit",
                "17",
                "--message",
                "hello",
                "--message-file",
                "-",
            ],
            vec!["openproject", "comment", "0", "--message", "hello"],
            vec!["openproject", "comment", "edit", "0", "--message", "hello"],
        ] {
            assert!(Cli::try_parse_from(&args).is_err(), "accepted {args:?}");
        }
    }

    #[test]
    fn comment_input_preserves_markdown_and_rejects_blank_or_invalid_utf8() {
        let markdown = "  ## Résumé\n\n```rust\nlet n = 1;\n```\n";
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("comment.md");
        fs::write(&path, markdown).unwrap();
        let file_args = MessageArgs {
            message: None,
            message_file: Some(path.clone()),
        };
        assert_eq!(file_args.read_from(&mut io::empty()).unwrap(), markdown);
        let stdin_args = MessageArgs {
            message: None,
            message_file: Some(PathBuf::from("-")),
        };
        assert_eq!(
            stdin_args.read_from(&mut markdown.as_bytes()).unwrap(),
            markdown
        );
        assert!(stdin_args.read_from(&mut " \n\t".as_bytes()).is_err());
        assert!(stdin_args.read_from(&mut &[0xff_u8][..]).is_err());
        fs::write(&path, [0xff_u8]).unwrap();
        assert!(file_args.read_from(&mut io::empty()).is_err());
        fs::remove_file(path).unwrap();
        assert!(file_args.read_from(&mut io::empty()).is_err());
    }

    #[test]
    fn filtered_comment_pages_scan_history_deduplicate_and_include_change_comments() {
        for offset in ["1", "2"] {
            let cli = Cli::try_parse_from([
                "openproject",
                "comments",
                "42",
                "--author",
                "me",
                "--since",
                "7d",
                "--limit",
                "1",
                "--offset",
                offset,
            ])
            .unwrap();
            let mut old = activity(4, 7, "old comment");
            old["createdAt"] = json!("1999-01-01T00:00:00Z");
            let (client, server) = mock_server(vec![
                ("GET /api/v3/users/me", 200, json!({"id": 7})),
                (
                    "GET /api/v3/work_packages/42/activities?pageSize=100&offset=1",
                    200,
                    json!({
                        "_embedded": {"elements": [{"id": 1}, {"id": 2}]},
                        "_links": {"nextByOffset": {"href": "/api/v3/work_packages/42/activities?pageSize=100&offset=2"}},
                    }),
                ),
                ("GET /api/v3/activities/1", 200, activity(1, 7, " \n")),
                (
                    "GET /api/v3/activities/2",
                    200,
                    activity(2, 8, "other author"),
                ),
                (
                    "GET /api/v3/work_packages/42/activities?pageSize=100&offset=2",
                    200,
                    json!({
                        "_embedded": {"elements": [{"id": 2}, {"id": 3}, {"id": 4}]},
                        "_links": {"nextByOffset": {"href": "/api/v3/work_packages/42/activities?pageSize=100&offset=3"}},
                    }),
                ),
                (
                    "GET /api/v3/activities/3",
                    200,
                    activity(3, 7, "comment on a change"),
                ),
                ("GET /api/v3/activities/4", 200, old),
                (
                    "GET /api/v3/work_packages/42/activities?pageSize=100&offset=3",
                    200,
                    json!({
                        "total": 5, "_embedded": {"elements": [{"id": 5}]}, "_links": {},
                    }),
                ),
                (
                    "GET /api/v3/activities/5",
                    200,
                    activity(5, 7, "second match"),
                ),
            ]);
            let result = list(&client, list_args(&cli)).unwrap();
            assert_eq!(result["count"], 1);
            assert_eq!(result["items"][0]["author"]["id"], 7);
            assert_eq!(result["items"][0]["internal"], true);
            if offset == "1" {
                assert_eq!(result["items"][0]["id"], 3);
                assert_eq!(result["nextOffset"], 2);
                assert!(result["total"].is_null());
            } else {
                assert_eq!(result["items"][0]["id"], 5);
                assert!(result["nextOffset"].is_null());
                assert_eq!(result["total"], 2);
            }
            assert_eq!(server.join().unwrap().len(), 9);
        }
    }

    #[test]
    fn comment_listing_handles_unpaginated_history_and_reports_broken_pagination() {
        let cli = Cli::try_parse_from(["openproject", "comments", "42"]).unwrap();
        for (page, expected_error) in [
            (
                json!({"total": 1, "_embedded": {"elements": [{"id": 3}]}}),
                None,
            ),
            (
                json!({"total": 2, "_embedded": {"elements": [{"id": 3}]}}),
                Some("missing its next-page link"),
            ),
            (
                json!({"_embedded": {"elements": [{"id": 3}]}, "_links": {"nextByOffset": {"href": "/work_packages/42/activities?pageSize=100&offset=1"}}}),
                Some("repeated activity page"),
            ),
        ] {
            let (client, server) = mock_server(vec![
                (
                    "GET /api/v3/work_packages/42/activities?pageSize=100&offset=1",
                    200,
                    page,
                ),
                (
                    "GET /api/v3/activities/3",
                    200,
                    activity(3, 7, "change comment"),
                ),
            ]);
            let result = list(&client, list_args(&cli));
            if let Some(error) = expected_error {
                assert!(result.unwrap_err().to_string().contains(error));
            } else {
                let result = result.unwrap();
                assert_eq!(result["total"], 1);
                assert_eq!(result["items"][0]["comment"]["raw"], "change comment");
                assert!(result["nextOffset"].is_null());
            }
            server.join().unwrap();
        }
    }

    #[test]
    fn comment_edit_sends_only_text_and_dry_run_never_patches() {
        for dry_run in [false, true] {
            let mut args = vec![
                "openproject",
                "comment",
                "edit",
                "3",
                "--message",
                "  this is up\ndated message\n",
            ];
            if dry_run {
                args.push("--dry-run");
            }
            let cli = Cli::try_parse_from(args).unwrap();
            let original = activity(3, 7, "original");
            let mut responses = vec![("GET /api/v3/activities/3", 200, original)];
            if !dry_run {
                responses.push((
                    "PATCH /api/v3/activities/3",
                    200,
                    activity(3, 7, "  this is up\ndated message\n"),
                ));
            }
            let (client, server) = mock_server(responses);
            let result = execute(&client, &cli, comment_args(&cli)).unwrap();
            let requests = server.join().unwrap();
            let payload = json!({"comment": "  this is up\ndated message\n"});
            if dry_run {
                assert_eq!(requests.len(), 1);
                assert_eq!(result["method"], "PATCH");
                assert_eq!(result["path"], "/activities/3");
                assert_eq!(result["payload"], payload);
            } else {
                assert_eq!(requests[1], payload);
                assert_eq!(result["internal"], true);
            }
        }
    }

    #[test]
    fn comment_edit_rejects_noncomments_missing_permission_and_server_denials() {
        let cli = Cli::try_parse_from([
            "openproject",
            "comment",
            "edit",
            "3",
            "--message",
            "corrected",
        ])
        .unwrap();
        let mut forbidden = activity(3, 7, "original");
        forbidden["_links"]
            .as_object_mut()
            .unwrap()
            .remove("update");
        for (original, expected) in [
            (activity(3, 7, ""), "no comment to edit"),
            (forbidden, "current permissions"),
        ] {
            let (client, server) = mock_server(vec![("GET /api/v3/activities/3", 200, original)]);
            assert!(execute(&client, &cli, comment_args(&cli))
                .unwrap_err()
                .to_string()
                .contains(expected));
            assert_eq!(server.join().unwrap().len(), 1);
        }
        let (client, server) = mock_server(vec![
            ("GET /api/v3/activities/3", 200, activity(3, 7, "original")),
            (
                "PATCH /api/v3/activities/3",
                403,
                json!({"message": "not allowed to edit"}),
            ),
        ]);
        assert!(execute(&client, &cli, comment_args(&cli))
            .unwrap_err()
            .to_string()
            .contains("HTTP 403"));
        assert_eq!(server.join().unwrap().len(), 2);
    }

    #[test]
    fn new_comment_posts_markdown_and_dry_run_sends_no_request() {
        let cli = Cli::try_parse_from([
            "openproject",
            "comment",
            "42",
            "--message",
            "**progress**\n",
        ])
        .unwrap();
        let payload = json!({"comment": {"format": "markdown", "raw": "**progress**\n"}});
        let (client, server) = mock_server(vec![(
            "POST /api/v3/work_packages/42/activities",
            201,
            activity(3, 7, "**progress**\n"),
        )]);
        assert_eq!(execute(&client, &cli, comment_args(&cli)).unwrap()["id"], 3);
        assert_eq!(server.join().unwrap()[0], payload);

        let cli = Cli::try_parse_from([
            "openproject",
            "comment",
            "42",
            "--message",
            "**progress**\n",
            "--dry-run",
        ])
        .unwrap();
        let client =
            OpenProjectClient::new("http://127.0.0.1:1".to_owned(), "test-token".to_owned())
                .unwrap();
        let result = execute(&client, &cli, comment_args(&cli)).unwrap();
        assert_eq!(result["method"], "POST");
        assert_eq!(result["payload"], payload);
    }

    #[test]
    fn since_requires_a_positive_bounded_day_count() {
        let now = DateTime::parse_from_rfc3339("2026-10-10T12:00:00+08:00")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(since_cutoff("7d", now).unwrap(), now - Duration::days(7));
        assert_eq!(since_cutoff("7", now).unwrap(), now - Duration::days(7));
        for value in ["0", "-1", "1h", "yesterday", "9223372036854775807"] {
            assert!(since_cutoff(value, now).is_err(), "accepted {value}");
        }
    }
}
