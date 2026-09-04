use anyhow::{anyhow, bail, Context, Result};
use chrono::Local;
use clap::{Args, Parser, Subcommand};
use regex::Regex;
use reqwest::blocking::{Client as HttpClient, RequestBuilder};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::process::Stdio;
use std::process::{Command, ExitCode};
use url::Url;

const API_ACCEPT: &str = "application/hal+json, application/json";

#[derive(Parser, Debug)]
#[command(
    name = "openproject",
    version,
    about = "Portable OpenProject API v3 client"
)]
struct Cli {
    /// OpenProject base URL. See README for configuration precedence.
    #[arg(long, global = true)]
    host: Option<String>,
    /// Repository directory used for project discovery.
    #[arg(long, global = true, default_value = ".")]
    cwd: PathBuf,
    /// Emit machine-readable JSON.
    #[arg(long, global = true)]
    json: bool,
    /// Preview mutations without applying them.
    #[arg(long, global = true)]
    dry_run: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Confirm that OPENPROJECT_URL and OPENPROJECT_TOKEN work.
    Auth {
        #[command(subcommand)]
        command: AuthCommands,
    },
    /// List visible OpenProject projects.
    Projects,
    /// Resolve the project for this repository.
    Project(ProjectArg),
    /// List work packages in a project.
    Tasks(TasksArgs),
    /// Show one work package.
    Task { task_id: u64 },
    /// Create a work package.
    Create(CreateArgs),
    /// Update a work package.
    Update(UpdateArgs),
    /// Add an activity comment.
    Comment {
        task_id: u64,
        #[arg(long)]
        message: String,
    },
    /// Log time against a work package.
    LogTime(LogTimeArgs),
    /// Build a safe clickable link for a commit in the current Git repository.
    CommitLink(CommitLinkArgs),
    /// Remove this OpenProject executable. Configuration and Agent Skill files are preserved.
    Uninstall,
}

#[derive(Subcommand, Debug)]
enum AuthCommands {
    Verify,
}

#[derive(Args, Debug)]
struct ProjectArg {
    #[arg(long)]
    project: Option<String>,
}

#[derive(Args, Debug)]
struct TasksArgs {
    #[arg(long)]
    project: Option<String>,
    #[arg(long)]
    all: bool,
    #[arg(long)]
    assignee: Option<String>,
    #[arg(long)]
    query: Option<String>,
}

#[derive(Args, Debug)]
struct CreateArgs {
    #[arg(long)]
    project: Option<String>,
    #[arg(long)]
    subject: String,
    #[arg(long)]
    description: Option<String>,
    #[arg(long, default_value = "Task")]
    r#type: String,
    #[arg(long)]
    type_id: Option<u64>,
    #[arg(long)]
    assignee: Option<String>,
    #[arg(long)]
    start_date: Option<String>,
    #[arg(long)]
    due_date: Option<String>,
    #[arg(long)]
    estimate: Option<String>,
}

#[derive(Args, Debug)]
struct UpdateArgs {
    task_id: u64,
    #[arg(long)]
    subject: Option<String>,
    #[arg(long)]
    description: Option<String>,
    #[arg(long)]
    status: Option<String>,
    #[arg(long)]
    assignee: Option<String>,
    #[arg(long, value_parser = clap::value_parser!(u8).range(0..=100))]
    percent: Option<u8>,
    #[arg(long)]
    start_date: Option<String>,
    #[arg(long)]
    due_date: Option<String>,
    #[arg(long)]
    estimate: Option<String>,
}

#[derive(Args, Debug)]
struct LogTimeArgs {
    task_id: u64,
    #[arg(long)]
    hours: String,
    #[arg(long, default_value_t = Local::now().date_naive().to_string())]
    date: String,
    #[arg(long)]
    comment: Option<String>,
    #[arg(long)]
    activity_id: Option<u64>,
}

#[derive(Args, Debug)]
struct CommitLinkArgs {
    commit: String,
    #[arg(long, default_value = "origin")]
    remote: String,
    #[arg(long, default_value = "html", value_parser = ["html", "url", "json"])]
    format: String,
}

struct OpenProjectClient {
    host: String,
    base: String,
    http: HttpClient,
    token: String,
}

impl OpenProjectClient {
    fn new(host: String, token: String) -> Result<Self> {
        let parsed =
            Url::parse(&host).context("OPENPROJECT_URL must be an absolute http(s) URL")?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            bail!("OPENPROJECT_URL must be an absolute http(s) URL");
        }
        let host = host.trim_end_matches('/').to_owned();
        Ok(Self {
            base: format!("{host}/api/v3"),
            host,
            http: HttpClient::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()?,
            token,
        })
    }
    fn url(&self, path: &str) -> Result<String> {
        if path.starts_with("http://") || path.starts_with("https://") {
            let url = Url::parse(path)?;
            let expected = Url::parse(&self.host)?;
            if url.scheme() != expected.scheme()
                || url.host_str() != expected.host_str()
                || url.port_or_known_default() != expected.port_or_known_default()
            {
                bail!("refusing to send credentials to a different host");
            }
            return Ok(path.to_owned());
        }
        Ok(if path.starts_with("/api/v3/") || path == "/api/v3" {
            format!("{}{}", self.host, path)
        } else {
            format!("{}/{}", self.base, path.trim_start_matches('/'))
        })
    }
    fn request(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> Result<Value> {
        let url = self.url(path)?;
        let mut request: RequestBuilder = self
            .http
            .request(method, url)
            .header(ACCEPT, API_ACCEPT)
            .header(AUTHORIZATION, format!("Bearer {}", self.token));
        if let Some(payload) = body {
            request = request
                .header(CONTENT_TYPE, "application/json")
                .json(&payload);
        }
        let response = request.send().context("cannot connect to OpenProject")?;
        let status = response.status();
        let text = response.text().unwrap_or_default();
        if !status.is_success() {
            let detail = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| {
                    v.get("message")
                        .or_else(|| v.get("errorIdentifier"))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .unwrap_or(text);
            bail!("OpenProject HTTP {}: {}", status.as_u16(), detail);
        }
        if text.trim().is_empty() {
            return Ok(json!({}));
        }
        serde_json::from_str(&text).context("OpenProject returned invalid JSON")
    }
    fn get(&self, path: &str) -> Result<Value> {
        self.request(reqwest::Method::GET, path, None)
    }
    fn collection(&self, path: &str) -> Result<Vec<Value>> {
        let mut next = format!("{}?pageSize=100", path);
        let mut items = Vec::new();
        loop {
            let page = self.get(&next)?;
            if let Some(elements) = page
                .pointer("/_embedded/elements")
                .and_then(Value::as_array)
            {
                items.extend(elements.iter().cloned());
            }
            match page
                .pointer("/_links/nextByOffset/href")
                .and_then(Value::as_str)
            {
                Some(link) => next = link.to_owned(),
                None => break,
            }
        }
        Ok(items)
    }
}

#[derive(Debug, Default, PartialEq)]
struct Config {
    host: Option<String>,
    project: Option<String>,
}

fn global_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|path| path.join("openproject").join("config.json"))
}

fn project_config_path(cwd: &Path) -> PathBuf {
    git_root(cwd)
        .unwrap_or_else(|| cwd.to_path_buf())
        .join(".openproject.json")
}

fn read_config(path: &Path) -> Result<Map<String, Value>> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Map::new()),
        Err(error) => return Err(error).with_context(|| format!("cannot read {}", path.display())),
    };
    parse_config(&contents, path)
}

fn parse_config(contents: &str, path: &Path) -> Result<Map<String, Value>> {
    serde_json::from_str::<Value>(contents)
        .with_context(|| format!("invalid JSON in {}", path.display()))?
        .as_object()
        .cloned()
        .ok_or_else(|| anyhow!("configuration in {} must be a JSON object", path.display()))
}

fn validate_keys(settings: &Map<String, Value>, allowed: &[&str], path: &Path) -> Result<()> {
    if let Some(key) = settings.keys().find(|key| !allowed.contains(&key.as_str())) {
        bail!("unsupported setting {key:?} in {}", path.display());
    }
    Ok(())
}

fn host_setting(settings: &Map<String, Value>, path: &Path) -> Result<Option<String>> {
    settings
        .get("host")
        .map(|value| {
            value
                .as_str()
                .filter(|host| !host.trim().is_empty())
                .map(str::to_owned)
                .ok_or_else(|| anyhow!("host in {} must be a non-empty string", path.display()))
        })
        .transpose()
}

fn project_setting(settings: &Map<String, Value>, path: &Path) -> Result<Option<String>> {
    settings
        .get("project_id")
        .or_else(|| settings.get("project"))
        .map(|value| {
            value
                .as_str()
                .filter(|project| !project.trim().is_empty())
                .map(str::to_owned)
                .or_else(|| value.as_u64().filter(|id| *id > 0).map(|id| id.to_string()))
                .ok_or_else(|| {
                    anyhow!(
                        "project_id or project in {} must be a non-empty string or positive integer",
                        path.display()
                    )
                })
        })
        .transpose()
}

fn config_from_maps(
    global: &Map<String, Value>,
    global_path: &Path,
    project: &Map<String, Value>,
    project_path: &Path,
) -> Result<Config> {
    validate_keys(global, &["host"], global_path)?;
    validate_keys(project, &["host", "project_id", "project"], project_path)?;
    Ok(Config {
        host: host_setting(project, project_path)?.or(host_setting(global, global_path)?),
        project: project_setting(project, project_path)?,
    })
}

fn config(cwd: &Path) -> Result<Config> {
    let global_path = global_config_path();
    let global = global_path
        .as_deref()
        .map(read_config)
        .transpose()?
        .unwrap_or_default();
    let project_path = project_config_path(cwd);
    let project = read_config(&project_path)?;
    let fallback_global_path = Path::new("<global config unavailable>");
    config_from_maps(
        &global,
        global_path.as_deref().unwrap_or(fallback_global_path),
        &project,
        &project_path,
    )
}

fn resolve_host(
    cli_host: Option<&str>,
    env_host: Option<String>,
    config: &Config,
) -> Result<String> {
    cli_host
        .map(str::to_owned)
        .or(env_host)
        .or_else(|| config.host.clone())
        .ok_or_else(|| anyhow!("set OPENPROJECT_URL, pass --host, or configure a host"))
}

fn git_root(cwd: &Path) -> Option<PathBuf> {
    git(cwd, &["rev-parse", "--show-toplevel"])
        .ok()
        .map(PathBuf::from)
}
fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .context("git is not available")?;
    if !out.status.success() {
        bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}
fn normalize(value: &str) -> String {
    value
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
fn href<'a>(value: &'a Value, name: &str) -> Option<&'a str> {
    value
        .pointer(&format!("/_links/{name}/href"))
        .and_then(Value::as_str)
}
fn title<'a>(value: &'a Value, name: &str) -> Option<&'a str> {
    value
        .pointer(&format!("/_links/{name}/title"))
        .and_then(Value::as_str)
}
fn id(value: &Value) -> Result<u64> {
    value
        .get("id")
        .and_then(Value::as_u64)
        .ok_or_else(|| anyhow!("OpenProject response has no numeric id"))
}

fn resolve_project(
    client: &OpenProjectClient,
    cwd: &Path,
    settings: &Config,
    explicit: Option<&str>,
) -> Result<Value> {
    let value = explicit
        .map(str::to_owned)
        .or_else(|| settings.project.clone());
    let projects = client.collection("/projects")?;
    if let Some(value) = value {
        if let Ok(number) = value.parse::<u64>() {
            return client.get(&format!("/projects/{number}"));
        }
        let matches: Vec<_> = projects
            .into_iter()
            .filter(|p| {
                [p.get("name"), p.get("identifier")]
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .any(|x| normalize(x) == normalize(&value))
            })
            .collect();
        return match matches.len() {
            1 => Ok(matches.into_iter().next().unwrap()),
            0 => bail!("no OpenProject project exactly matches {value:?}"),
            _ => bail!("multiple OpenProject projects match {value:?}; use a numeric --project"),
        };
    }
    let root = git_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    let repo = root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let matches: Vec<_> = projects
        .into_iter()
        .filter(|p| {
            [p.get("name"), p.get("identifier")]
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .any(|x| normalize(x) == normalize(repo))
        })
        .collect();
    match matches.len() { 1 => Ok(matches.into_iter().next().unwrap()), _ => bail!("cannot resolve this repository to one project safely; use --project or .openproject.json") }
}

fn resolve_item(client: &OpenProjectClient, path: &str, value: &str, kind: &str) -> Result<u64> {
    if let Ok(n) = value.parse() {
        return Ok(n);
    }
    let matches: Vec<_> = client
        .collection(path)?
        .into_iter()
        .filter(|item| {
            item.get("name")
                .and_then(Value::as_str)
                .map(|name| normalize(name) == normalize(value))
                .unwrap_or(false)
        })
        .collect();
    match matches.len() {
        1 => id(&matches[0]),
        0 => bail!("no {kind} exactly matches {value:?}"),
        _ => bail!("multiple {kind} values match {value:?}; use a numeric ID"),
    }
}
fn resolve_user(client: &OpenProjectClient, value: &str) -> Result<u64> {
    if value.eq_ignore_ascii_case("me") {
        return id(&client.get("/users/me")?);
    }
    value
        .parse()
        .map_err(|_| anyhow!("assignee must be a numeric user ID or 'me'"))
}
fn duration(value: &str) -> Result<String> {
    let upper = value.to_ascii_uppercase();
    if Regex::new(r"^P(?:\d+D)?(?:T(?:\d+H)?(?:\d+M)?)?$")?.is_match(&upper)
        && upper != "P"
        && upper != "PT"
    {
        return Ok(upper);
    }
    let hours: f64 = value
        .parse()
        .context("hours must be decimal hours or an ISO-8601 duration")?;
    if hours <= 0.0 || (hours * 60.0).fract().abs() > f64::EPSILON {
        bail!("hours must be positive and resolve to whole minutes");
    }
    let total = (hours * 60.0) as u64;
    Ok(format!("PT{}H{}M", total / 60, total % 60))
}
fn task_summary(task: &Value, host: &str) -> Value {
    let task_id = task.get("id").and_then(Value::as_u64);
    json!({"id":task_id,"subject":task.get("subject"),"status":title(task,"status"),"type":title(task,"type"),"assignee":title(task,"assignee"),"percentageDone":task.get("percentageDone"),"spentTime":task.get("spentTime"),"startDate":task.get("startDate"),"dueDate":task.get("dueDate"),"url":task_id.map(|n|format!("{host}/work_packages/{n}"))})
}
fn emit(value: Value, as_json: bool) {
    if as_json {
        println!("{}", serde_json::to_string_pretty(&value).unwrap());
    } else if let Some(items) = value.as_array() {
        for item in items {
            println!(
                "#{} {}",
                item.get("id")
                    .and_then(Value::as_u64)
                    .map(|n| n.to_string())
                    .unwrap_or_default(),
                item.get("subject")
                    .or_else(|| item.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            );
        }
    } else {
        println!("{}", serde_json::to_string_pretty(&value).unwrap());
    }
}
fn compact(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .filter_map(|(key, value)| {
                    if value.is_null() {
                        None
                    } else {
                        Some((key, compact(value)))
                    }
                })
                .collect(),
        ),
        Value::Array(values) => Value::Array(
            values
                .into_iter()
                .filter(|value| !value.is_null())
                .map(compact)
                .collect(),
        ),
        other => other,
    }
}
fn write(
    client: &OpenProjectClient,
    cli: &Cli,
    method: reqwest::Method,
    path: &str,
    body: Value,
) -> Result<Value> {
    let body = compact(body);
    if cli.dry_run {
        Ok(json!({"dryRun":true,"method":method.as_str(),"path":path,"payload":body}))
    } else {
        client.request(method, path, Some(body))
    }
}

fn commit_link(cwd: &Path, args: &CommitLinkArgs) -> Result<Value> {
    let commit = git(
        cwd,
        &[
            "rev-parse",
            "--verify",
            &format!("{}^{{commit}}", args.commit),
        ],
    )?;
    let short = git(cwd, &["rev-parse", "--short=8", &commit])?;
    let remote = git(cwd, &["remote", "get-url", &args.remote])?;
    let (base, host) = if remote.contains("://") {
        let url = Url::parse(&remote)?;
        let host = url
            .host_str()
            .ok_or_else(|| anyhow!("Git remote URL has no hostname"))?
            .to_owned();
        (
            format!(
                "https://{}/{}",
                host,
                url.path().trim_matches('/').trim_end_matches(".git")
            ),
            host,
        )
    } else {
        let re = Regex::new(r"^(?:[^@]+@)?([^:]+):(.+)$")?;
        let cap = re
            .captures(&remote)
            .ok_or_else(|| anyhow!("unsupported Git remote URL format"))?;
        (
            format!("https://{}/{}", &cap[1], cap[2].trim_end_matches(".git")),
            cap[1].to_owned(),
        )
    };
    let route = if host.contains("gitlab") {
        "/-/commit/"
    } else if host.contains("github") || host.contains("gitea") {
        "/commit/"
    } else if host.contains("bitbucket") {
        "/commits/"
    } else {
        bail!("cannot determine commit route for Git host {host:?}");
    };
    let url = format!("{base}{route}{commit}");
    Ok(
        json!({"commit":commit,"shortCommit":short,"repository":base,"url":url,"html":format!("<a href=\"{}\"><code>{}</code></a>", url, short)}),
    )
}

#[cfg(not(windows))]
fn remove_current_executable(path: &Path) -> Result<&'static str> {
    fs::remove_file(path)
        .with_context(|| format!("cannot remove executable {}", path.display()))?;
    Ok("removed")
}

#[cfg(windows)]
fn remove_current_executable(path: &Path) -> Result<&'static str> {
    let script = env::temp_dir().join(format!("openproject-uninstall-{}.cmd", std::process::id()));
    fs::write(
        &script,
        "@echo off\r\nfor /L %%i in (1,1,30) do (\r\n  del /f /q \"%OPENPROJECT_UNINSTALL_TARGET%\" >nul 2>&1\r\n  if not exist \"%OPENPROJECT_UNINSTALL_TARGET%\" goto done\r\n  ping 127.0.0.1 -n 2 >nul\r\n)\r\n:done\r\ndel /f /q \"%~f0\"\r\n",
    )
    .with_context(|| format!("cannot create uninstall helper {}", script.display()))?;
    let result = Command::new("cmd")
        .arg("/C")
        .arg(&script)
        .env("OPENPROJECT_UNINSTALL_TARGET", path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Err(error) = result {
        let _ = fs::remove_file(&script);
        return Err(error).context("cannot start Windows uninstall helper");
    }
    Ok("scheduled")
}

fn uninstall(cli: &Cli) -> Result<()> {
    let executable = env::current_exe().context("cannot locate the current executable")?;
    let path = executable.display().to_string();
    if cli.dry_run {
        if cli.json {
            emit(
                json!({"dryRun":true,"operation":"uninstall","path":path}),
                true,
            );
        } else {
            println!("Would remove {path}");
        }
        return Ok(());
    }

    let status = remove_current_executable(&executable)?;
    if cli.json {
        emit(json!({"path":path,"status":status}), true);
    } else if status == "scheduled" {
        println!("Scheduled removal of {path} after this process exits");
    } else {
        println!("Removed {path}");
    }
    Ok(())
}

fn run(cli: &Cli) -> Result<()> {
    if let Commands::Uninstall = &cli.command {
        return uninstall(cli);
    }
    if let Commands::CommitLink(args) = &cli.command {
        let result = commit_link(&cli.cwd, args)?;
        match args.format.as_str() {
            "url" => println!("{}", result["url"].as_str().unwrap()),
            "html" => println!("{}", result["html"].as_str().unwrap()),
            _ => emit(result, true),
        };
        return Ok(());
    }
    let cfg = config(&cli.cwd)?;
    let host = resolve_host(cli.host.as_deref(), env::var("OPENPROJECT_URL").ok(), &cfg)?;
    let token = env::var("OPENPROJECT_TOKEN")
        .context("set OPENPROJECT_TOKEN to an OpenProject API token")?;
    let client = OpenProjectClient::new(host, token)?;
    match &cli.command {
        Commands::Auth {
            command: AuthCommands::Verify,
        } => emit(client.get("/users/me")?, cli.json),
        Commands::Projects => emit(Value::Array(client.collection("/projects")?), cli.json),
        Commands::Project(args) => emit(
            resolve_project(&client, &cli.cwd, &cfg, args.project.as_deref())?,
            cli.json,
        ),
        Commands::Tasks(args) => {
            let project = resolve_project(&client, &cli.cwd, &cfg, args.project.as_deref())?;
            let project_id = id(&project)?;
            let mut tasks = client.collection(&format!("/projects/{project_id}/work_packages"))?;
            if let Some(query) = &args.query {
                tasks.retain(|t| {
                    t.get("subject")
                        .and_then(Value::as_str)
                        .map(|s| s.to_ascii_lowercase().contains(&query.to_ascii_lowercase()))
                        .unwrap_or(false)
                });
            }
            if let Some(assignee) = &args.assignee {
                let wanted = resolve_user(&client, assignee)?;
                let expected = format!("/api/v3/users/{wanted}");
                tasks.retain(|t| href(t, "assignee") == Some(expected.as_str()));
            }
            if !args.all {
                let closed: HashSet<u64> = client
                    .collection("/statuses")?
                    .iter()
                    .filter(|s| s.get("isClosed").and_then(Value::as_bool) == Some(true))
                    .filter_map(|s| s.get("id").and_then(Value::as_u64))
                    .collect();
                tasks.retain(|t| {
                    href(t, "status")
                        .and_then(|h| h.rsplit('/').next())
                        .and_then(|n| n.parse().ok())
                        .map(|n| !closed.contains(&n))
                        .unwrap_or(true)
                });
            }
            emit(
                Value::Array(
                    tasks
                        .iter()
                        .map(|t| task_summary(t, &client.host))
                        .collect(),
                ),
                cli.json,
            );
        }
        Commands::Task { task_id } => emit(
            task_summary(
                &client.get(&format!("/work_packages/{task_id}"))?,
                &client.host,
            ),
            cli.json,
        ),
        Commands::Create(args) => {
            let project = resolve_project(&client, &cli.cwd, &cfg, args.project.as_deref())?;
            let type_id =
                args.type_id
                    .unwrap_or(resolve_item(&client, "/types", &args.r#type, "type")?);
            let assignee = args
                .assignee
                .as_deref()
                .map(|a| resolve_user(&client, a))
                .transpose()?;
            let payload = json!({"subject":args.subject,"description":args.description.as_ref().map(|raw|json!({"format":"markdown","raw":raw})),"startDate":args.start_date,"dueDate":args.due_date,"estimatedTime":args.estimate.as_deref().map(duration).transpose()?,"_links":{"project":{"href":format!("/api/v3/projects/{}",id(&project)?)},"type":{"href":format!("/api/v3/types/{type_id}")},"assignee":assignee.map(|n|json!({"href":format!("/api/v3/users/{n}")}))}});
            emit(
                write(
                    &client,
                    cli,
                    reqwest::Method::POST,
                    "/work_packages",
                    payload,
                )?,
                cli.json,
            );
        }
        Commands::Update(args) => {
            let current = client.get(&format!("/work_packages/{}", args.task_id))?;
            let status = args
                .status
                .as_deref()
                .map(|s| resolve_item(&client, "/statuses", s, "status"))
                .transpose()?;
            let assignee = args
                .assignee
                .as_deref()
                .map(|a| resolve_user(&client, a))
                .transpose()?;
            if args.subject.is_none()
                && args.description.is_none()
                && status.is_none()
                && assignee.is_none()
                && args.percent.is_none()
                && args.start_date.is_none()
                && args.due_date.is_none()
                && args.estimate.is_none()
            {
                bail!("no update fields were supplied");
            }
            let mut links = Map::new();
            if let Some(n) = status {
                links.insert(
                    "status".into(),
                    json!({"href":format!("/api/v3/statuses/{n}")}),
                );
            }
            if let Some(n) = assignee {
                links.insert(
                    "assignee".into(),
                    json!({"href":format!("/api/v3/users/{n}")}),
                );
            }
            let payload = json!({"lockVersion":current.get("lockVersion"),"subject":args.subject,"description":args.description.as_ref().map(|raw|json!({"format":"markdown","raw":raw})),"percentageDone":args.percent,"startDate":args.start_date,"dueDate":args.due_date,"estimatedTime":args.estimate.as_deref().map(duration).transpose()?,"_links":links});
            emit(
                write(
                    &client,
                    cli,
                    reqwest::Method::PATCH,
                    &format!("/work_packages/{}", args.task_id),
                    payload,
                )?,
                cli.json,
            );
        }
        Commands::Comment { task_id, message } => emit(
            write(
                &client,
                cli,
                reqwest::Method::POST,
                &format!("/work_packages/{task_id}/activities"),
                json!({"comment":{"format":"markdown","raw":message}}),
            )?,
            cli.json,
        ),
        Commands::LogTime(args) => {
            chrono::NaiveDate::parse_from_str(&args.date, "%Y-%m-%d")
                .context("date must be YYYY-MM-DD")?;
            let task = client.get(&format!("/work_packages/{}", args.task_id))?;
            let project = href(&task, "project")
                .ok_or_else(|| anyhow!("work package response has no project link"))?;
            let payload = json!({"hours":duration(&args.hours)?,"spentOn":args.date,"comment":args.comment.as_ref().map(|raw|json!({"format":"plain","raw":raw})),"_links":{"workPackage":{"href":format!("/api/v3/work_packages/{}",args.task_id)},"project":{"href":project},"activity":args.activity_id.map(|n|json!({"href":format!("/api/v3/time_entries/activities/{n}")}))}});
            emit(
                write(
                    &client,
                    cli,
                    reqwest::Method::POST,
                    "/time_entries",
                    payload,
                )?,
                cli.json,
            );
        }
        Commands::CommitLink(_) | Commands::Uninstall => unreachable!(),
    };
    Ok(())
}

fn error_payload(error: &anyhow::Error) -> Value {
    json!({"error":{"message":format!("{error:#}")}})
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            if cli.json {
                eprintln!(
                    "{}",
                    serde_json::to_string(&error_payload(&error))
                        .expect("error payload is serializable")
                );
            } else {
                eprintln!("Error: {error:#}");
            }
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::error::ErrorKind as ClapErrorKind;

    fn object(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn reports_cargo_package_version() {
        for flag in ["--version", "-V"] {
            let error = Cli::try_parse_from(["openproject", flag]).unwrap_err();
            assert_eq!(error.kind(), ClapErrorKind::DisplayVersion);
            assert_eq!(
                error.to_string(),
                format!("openproject {}\n", env!("CARGO_PKG_VERSION"))
            );
        }
    }

    #[test]
    fn uninstall_dry_run_does_not_require_openproject_credentials() {
        let cli = Cli::try_parse_from(["openproject", "uninstall", "--dry-run", "--json"]).unwrap();
        assert!(run(&cli).is_ok());
    }

    #[test]
    fn project_config_overrides_global_host() {
        let global = object(json!({"host":"https://global.example.com"}));
        let project = object(json!({"host":"https://project.example.com","project_id":13}));
        let config = config_from_maps(
            &global,
            Path::new("global.json"),
            &project,
            Path::new(".openproject.json"),
        )
        .unwrap();
        assert_eq!(
            config,
            Config {
                host: Some("https://project.example.com".into()),
                project: Some("13".into())
            }
        );
    }

    #[test]
    fn global_config_rejects_project_settings() {
        let global = object(json!({"host":"https://global.example.com","project_id":13}));
        let error = config_from_maps(
            &global,
            Path::new("global.json"),
            &Map::new(),
            Path::new(".openproject.json"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("project_id"));
        assert!(error.to_string().contains("global.json"));
    }

    #[test]
    fn project_id_takes_precedence_over_project_alias() {
        let project = object(json!({"project_id":13,"project":"legacy"}));
        let config = config_from_maps(
            &Map::new(),
            Path::new("global.json"),
            &project,
            Path::new(".openproject.json"),
        )
        .unwrap();
        assert_eq!(config.project.as_deref(), Some("13"));
    }

    #[test]
    fn host_precedence_is_cli_then_environment_then_config() {
        let config = Config {
            host: Some("https://config.example.com".into()),
            project: None,
        };
        assert_eq!(
            resolve_host(
                Some("https://cli.example.com"),
                Some("https://env.example.com".into()),
                &config,
            )
            .unwrap(),
            "https://cli.example.com"
        );
        assert_eq!(
            resolve_host(None, Some("https://env.example.com".into()), &config).unwrap(),
            "https://env.example.com"
        );
        assert_eq!(
            resolve_host(None, None, &config).unwrap(),
            "https://config.example.com"
        );
    }

    #[test]
    fn malformed_config_reports_its_path() {
        let error = parse_config("{", Path::new("broken.json")).unwrap_err();
        assert!(format!("{error:#}").contains("invalid JSON in broken.json"));
    }

    #[test]
    fn missing_config_is_empty() {
        let config = read_config(Path::new(
            "target/openproject-test-config-does-not-exist.json",
        ))
        .unwrap();
        assert!(config.is_empty());
    }

    #[test]
    fn json_errors_have_a_stable_agent_friendly_shape() {
        let error = anyhow!("configuration failed").context("cannot start");
        assert_eq!(
            error_payload(&error),
            json!({"error":{"message":"cannot start: configuration failed"}})
        );
    }

    #[test]
    fn decimal_duration_is_iso8601() {
        assert_eq!(duration("1.5").unwrap(), "PT1H30M");
    }
    #[test]
    fn rejects_fractional_minutes() {
        assert!(duration("0.01").is_err());
    }
    #[test]
    fn normalizes_project_names() {
        assert_eq!(normalize("My_Project!"), "my project");
    }
}
