use anyhow::{anyhow, bail, Context, Result};
use chrono::Local;
use clap::{Args, Parser, Subcommand};
use keyring::{Entry as KeyringEntry, Error as KeyringError};
use regex::Regex;
use reqwest::blocking::{Client as HttpClient, RequestBuilder};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::ErrorKind;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use url::Url;

const API_ACCEPT: &str = "application/hal+json, application/json";
const DEFAULT_RELEASE_REPOSITORY: &str = "yungts97/openproject-skill";
const CREDENTIAL_SERVICE: &str = "openproject-cli";
const EXAMPLE_HOST: &str = "https://openproject.example.com";

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
    /// Upgrade this executable from the configured GitHub release repository.
    Upgrade(UpgradeArgs),
    /// Remove this OpenProject executable. Configuration and Agent Skill files are preserved.
    Uninstall,
}

#[derive(Subcommand, Debug)]
enum AuthCommands {
    /// Interactively verify and securely save an OpenProject API token.
    Login,
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
struct UpgradeArgs {
    /// Release version to install, or "latest".
    #[arg(default_value = "latest")]
    version: String,
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
        let host = canonical_host(&host)?;
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

fn write_global_host(host: &str) -> Result<()> {
    let path = global_config_path()
        .ok_or_else(|| anyhow!("cannot determine the global config directory"))?;
    let mut settings = read_config(&path)?;
    validate_keys(&settings, &["host"], &path)?;
    settings.insert("host".into(), Value::String(host.to_owned()));
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("global config path has no parent directory"))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("cannot create global config directory {}", parent.display()))?;
    let contents = serde_json::to_string_pretty(&Value::Object(settings))?;
    fs::write(&path, format!("{contents}\n"))
        .with_context(|| format!("cannot write {}", path.display()))
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
        .and_then(|host| canonical_host(&host))
}

fn canonical_host(value: &str) -> Result<String> {
    let mut parsed =
        Url::parse(value.trim()).context("OpenProject URL must be an absolute http(s) URL")?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        bail!("OpenProject URL must be an absolute http(s) URL");
    }
    parsed.set_query(None);
    parsed.set_fragment(None);
    let path = parsed.path().trim_end_matches('/').to_owned();
    parsed.set_path(&path);
    Ok(parsed.as_str().trim_end_matches('/').to_owned())
}

fn credential_scope(host: &str) -> Result<String> {
    let host = canonical_host(host)?;
    Ok(url::form_urlencoded::byte_serialize(host.as_bytes()).collect())
}

trait CredentialStore {
    fn name(&self) -> &'static str;
    fn load(&self) -> Result<Option<String>>;
    fn save(&self, token: &str) -> Result<()>;
}

struct NativeCredentialStore {
    scope: String,
}

impl NativeCredentialStore {
    fn new(scope: String) -> Self {
        Self { scope }
    }

    fn entry(&self) -> Result<KeyringEntry> {
        KeyringEntry::new(CREDENTIAL_SERVICE, &self.scope)
            .context("cannot access the system credential store")
    }

    fn available() -> bool {
        KeyringEntry::store_status().is_ok()
    }
}

impl CredentialStore for NativeCredentialStore {
    fn name(&self) -> &'static str {
        "system credential store"
    }

    fn load(&self) -> Result<Option<String>> {
        match self.entry()?.get_password() {
            Ok(token) => Ok(Some(token)),
            Err(KeyringError::NoEntry) => Ok(None),
            Err(error) => Err(anyhow!(error)).context("cannot read the stored OpenProject token"),
        }
    }

    fn save(&self, token: &str) -> Result<()> {
        self.entry()?
            .set_password(token)
            .context("cannot save the OpenProject token in the system credential store")
    }
}

struct PassCredentialStore {
    entry: String,
}

impl PassCredentialStore {
    fn new(scope: String) -> Self {
        Self {
            entry: format!("{CREDENTIAL_SERVICE}/{scope}"),
        }
    }

    fn available() -> bool {
        Command::new("pass")
            .arg("ls")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }
}

impl CredentialStore for PassCredentialStore {
    fn name(&self) -> &'static str {
        "pass password store"
    }

    fn load(&self) -> Result<Option<String>> {
        let output = Command::new("pass")
            .arg("show")
            .arg(&self.entry)
            .stdin(Stdio::null())
            .output()
            .context("cannot run pass")?;
        if !output.status.success() {
            return Ok(None);
        }
        let token = String::from_utf8(output.stdout)
            .context("pass returned a token that is not valid UTF-8")?
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_owned();
        Ok((!token.is_empty()).then_some(token))
    }

    fn save(&self, token: &str) -> Result<()> {
        let mut command = Command::new("pass");
        command
            .arg("insert")
            .arg("--multiline")
            .arg("--force")
            .arg(&self.entry)
            .stdin(Stdio::piped())
            .stdout(Stdio::null());
        let mut child = command.spawn().context("cannot run pass")?;
        child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("cannot write the token to pass"))?
            .write_all(format!("{token}\n").as_bytes())
            .context("cannot write the token to pass")?;
        let status = child.wait().context("cannot wait for pass")?;
        if !status.success() {
            bail!("pass could not save the OpenProject token")
        }
        Ok(())
    }
}

fn credential_stores(host: &str) -> Vec<Box<dyn CredentialStore>> {
    let Ok(scope) = credential_scope(host) else {
        return Vec::new();
    };
    let mut stores: Vec<Box<dyn CredentialStore>> = Vec::new();
    if NativeCredentialStore::available() {
        stores.push(Box::new(NativeCredentialStore::new(scope.clone())));
    }
    if PassCredentialStore::available() {
        stores.push(Box::new(PassCredentialStore::new(scope)));
    }
    stores
}

fn first_stored_token(stores: &[Box<dyn CredentialStore>]) -> Option<String> {
    stores.iter().find_map(|store| store.load().ok().flatten())
}

fn token_from_sources(
    environment_token: Option<String>,
    stores: &[Box<dyn CredentialStore>],
) -> Option<String> {
    environment_token.or_else(|| first_stored_token(stores))
}

fn resolve_token(host: &str) -> Result<String> {
    token_from_sources(env::var("OPENPROJECT_TOKEN").ok(), &credential_stores(host)).ok_or_else(|| {
        anyhow!(
            "set OPENPROJECT_TOKEN for this session or run `openproject auth login` to save a token securely"
        )
    })
}

fn require_interactive_terminal() -> Result<()> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!("auth login requires an interactive terminal; set OPENPROJECT_TOKEN for non-interactive use")
    }
    Ok(())
}

fn prompt(label: &str) -> Result<String> {
    print!("{label}");
    io::stdout().flush().context("cannot write setup prompt")?;
    let mut value = String::new();
    io::stdin()
        .read_line(&mut value)
        .context("cannot read setup input")?;
    Ok(value.trim().to_owned())
}

fn prompt_confirmation(label: &str) -> Result<bool> {
    let response = prompt(label)?;
    Ok(response.is_empty() || matches!(response.as_str(), "y" | "Y" | "yes" | "YES"))
}

fn auth_login(cli: &Cli) -> Result<()> {
    if cli.json {
        bail!("auth login cannot be used with --json")
    }
    require_interactive_terminal()?;
    println!("OpenProject CLI setup\n");
    println!("[1/3] OpenProject server");
    let entered_host = match cli.host.as_deref() {
        Some(host) => host.to_owned(),
        None => prompt(&format!("OpenProject URL (for example, {EXAMPLE_HOST}): "))?,
    };
    if entered_host.is_empty() {
        bail!("an OpenProject URL is required")
    }
    let host = canonical_host(&entered_host)?;
    if host == EXAMPLE_HOST {
        bail!("replace the example URL with your real OpenProject server")
    }

    let stores = credential_stores(&host);
    let Some(primary_store) = stores.first() else {
        bail!(
            "no secure credential store is available. Configure a system credential manager or initialized pass store, or set OPENPROJECT_TOKEN for this session"
        )
    };
    println!("Credential storage: {}", primary_store.name());
    if first_stored_token(&stores).is_some()
        && !prompt_confirmation("A token is already saved for this server. Replace it? [y/N] ")?
    {
        println!("Setup cancelled; the existing token was kept.");
        return Ok(());
    }

    println!("\n[2/3] OpenProject API token");
    let token = rpassword::prompt_password("Token (input hidden): ")
        .context("cannot read the OpenProject API token")?;
    if token.trim().is_empty() {
        bail!("an OpenProject API token is required")
    }

    println!("\n[3/3] Verifying credentials");
    let client = OpenProjectClient::new(host.clone(), token.clone())?;
    client.get("/users/me")?;

    write_global_host(&host)?;
    let mut saved_by = None;
    for store in stores {
        if store.save(&token).is_ok() {
            saved_by = Some(store.name());
            break;
        }
    }
    let Some(saved_by) = saved_by else {
        bail!("the token was verified but could not be saved securely; set OPENPROJECT_TOKEN for this session and retry auth login after fixing your credential store")
    };
    println!("\nSetup complete. Host saved to global configuration; token saved in {saved_by}.");
    Ok(())
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

fn installer_url() -> String {
    let repository = env::var("OPENPROJECT_RELEASE_REPOSITORY")
        .unwrap_or_else(|_| DEFAULT_RELEASE_REPOSITORY.to_string());
    let installer = if cfg!(windows) {
        "install.ps1"
    } else {
        "install.sh"
    };
    format!(
        "https://raw.githubusercontent.com/{}/main/scripts/{installer}",
        repository.trim_matches('/')
    )
}

fn download_installer(url: &str, destination: &Path) -> Result<()> {
    let client = HttpClient::builder()
        .user_agent(format!("openproject/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .context("cannot create installer download client")?;
    let response = client
        .get(url)
        .send()
        .with_context(|| format!("cannot download installer from {url}"))?
        .error_for_status()
        .with_context(|| format!("installer download failed for {url}"))?;
    fs::write(destination, response.bytes()?)
        .with_context(|| format!("cannot write installer {}", destination.display()))
}

#[cfg(not(windows))]
fn run_upgrade_installer(
    cli: &Cli,
    installer: &Path,
    version: &str,
    destination: &Path,
) -> Result<()> {
    let mut command = Command::new("sh");
    command
        .arg(installer)
        .arg(version)
        .env("OPENPROJECT_INSTALL_DIR", destination);

    if cli.json {
        let output = command.output().context("cannot start upgrade installer")?;
        if !output.status.success() {
            let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
            bail!("upgrade installer failed: {message}");
        }
        emit(
            json!({
                "operation":"upgrade",
                "path":destination.join("openproject"),
                "status":"updated",
                "version":version
            }),
            true,
        );
    } else {
        let status = command.status().context("cannot start upgrade installer")?;
        if !status.success() {
            bail!("upgrade installer exited with {status}");
        }
    }
    Ok(())
}

#[cfg(windows)]
fn schedule_upgrade_installer(
    cli: &Cli,
    installer: &Path,
    version: &str,
    destination: &Path,
) -> Result<()> {
    let helper = env::temp_dir().join(format!(
        "openproject-upgrade-helper-{}.ps1",
        std::process::id()
    ));
    fs::write(
        &helper,
        r#"param(
  [int]$OpenProjectProcessId,
  [string]$InstallerPath,
  [string]$Version,
  [string]$Destination
)
$ErrorActionPreference = "Stop"
try {
  Wait-Process -Id $OpenProjectProcessId -ErrorAction SilentlyContinue
  & $InstallerPath -Version $Version -Destination $Destination
} finally {
  Remove-Item -LiteralPath $InstallerPath -Force -ErrorAction SilentlyContinue
  Remove-Item -LiteralPath $PSCommandPath -Force -ErrorAction SilentlyContinue
}
"#,
    )
    .with_context(|| format!("cannot create upgrade helper {}", helper.display()))?;

    let mut command = Command::new("powershell.exe");
    command
        .arg("-NoProfile")
        .arg("-ExecutionPolicy")
        .arg("Bypass")
        .arg("-File")
        .arg(&helper)
        .arg(std::process::id().to_string())
        .arg(installer)
        .arg(version)
        .arg(destination);
    if cli.json {
        command.stdout(Stdio::null()).stderr(Stdio::null());
    }
    if let Err(error) = command.spawn() {
        let _ = fs::remove_file(&helper);
        return Err(error).context("cannot start Windows upgrade helper");
    }

    let path = destination.join("openproject.exe");
    if cli.json {
        emit(
            json!({"operation":"upgrade","path":path,"status":"scheduled","version":version}),
            true,
        );
    } else {
        println!(
            "Scheduled upgrade of {} after this process exits",
            path.display()
        );
    }
    Ok(())
}

fn upgrade(cli: &Cli, args: &UpgradeArgs) -> Result<()> {
    let executable = env::current_exe().context("cannot locate the current executable")?;
    let destination = executable
        .parent()
        .ok_or_else(|| anyhow!("current executable has no parent directory"))?;
    let url = installer_url();
    let path = executable.display().to_string();
    if cli.dry_run {
        if cli.json {
            emit(
                json!({
                    "dryRun":true,
                    "installer":url,
                    "operation":"upgrade",
                    "path":path,
                    "version":args.version
                }),
                true,
            );
        } else {
            println!("Would upgrade {path} to {} using {url}", args.version);
        }
        return Ok(());
    }

    let extension = if cfg!(windows) { "ps1" } else { "sh" };
    let installer = env::temp_dir().join(format!(
        "openproject-upgrade-{}.{}",
        std::process::id(),
        extension
    ));
    download_installer(&url, &installer)?;

    #[cfg(windows)]
    {
        schedule_upgrade_installer(cli, &installer, &args.version, destination)
    }
    #[cfg(not(windows))]
    {
        let result = run_upgrade_installer(cli, &installer, &args.version, destination);
        let _ = fs::remove_file(&installer);
        result
    }
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
    if let Commands::Upgrade(args) = &cli.command {
        return upgrade(cli, args);
    }
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
    if let Commands::Auth {
        command: AuthCommands::Login,
    } = &cli.command
    {
        return auth_login(cli);
    }
    let cfg = config(&cli.cwd)?;
    let host = resolve_host(cli.host.as_deref(), env::var("OPENPROJECT_URL").ok(), &cfg)?;
    let token = resolve_token(&host)?;
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
        Commands::Auth {
            command: AuthCommands::Login,
        }
        | Commands::CommitLink(_)
        | Commands::Upgrade(_)
        | Commands::Uninstall => unreachable!(),
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
    use std::cell::RefCell;

    fn object(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    struct MemoryCredentialStore {
        token: RefCell<Option<String>>,
    }

    impl MemoryCredentialStore {
        fn empty() -> Self {
            Self {
                token: RefCell::new(None),
            }
        }
    }

    impl CredentialStore for MemoryCredentialStore {
        fn name(&self) -> &'static str {
            "memory"
        }

        fn load(&self) -> Result<Option<String>> {
            Ok(self.token.borrow().clone())
        }

        fn save(&self, token: &str) -> Result<()> {
            *self.token.borrow_mut() = Some(token.to_owned());
            Ok(())
        }
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
    fn upgrade_dry_run_does_not_require_openproject_credentials() {
        let cli = Cli::try_parse_from(["openproject", "upgrade", "0.2.0", "--dry-run", "--json"])
            .unwrap();
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
    fn invalid_configured_host_is_reported_before_token_lookup() {
        let config = Config {
            host: Some("not a URL".into()),
            project: None,
        };

        let error = resolve_host(None, None, &config).unwrap_err();

        assert!(error
            .to_string()
            .contains("OpenProject URL must be an absolute http(s) URL"));
    }

    #[test]
    fn canonical_host_removes_trailing_slashes_and_query_data() {
        assert_eq!(
            canonical_host("https://openproject.example.com/team/?ignored=value#fragment").unwrap(),
            "https://openproject.example.com/team"
        );
    }

    #[test]
    fn credential_scopes_are_isolated_by_host() {
        assert_ne!(
            credential_scope("https://one.example.com").unwrap(),
            credential_scope("https://two.example.com").unwrap()
        );
    }

    #[test]
    fn credential_store_can_save_and_load_a_token() {
        let store = MemoryCredentialStore::empty();
        store.save("stored-token").unwrap();
        assert_eq!(store.load().unwrap().as_deref(), Some("stored-token"));
    }

    #[test]
    fn environment_token_overrides_stored_token() {
        let store = MemoryCredentialStore::empty();
        store.save("stored-token").unwrap();
        let stores: Vec<Box<dyn CredentialStore>> = vec![Box::new(store)];
        assert_eq!(
            token_from_sources(Some("environment-token".into()), &stores).as_deref(),
            Some("environment-token")
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
