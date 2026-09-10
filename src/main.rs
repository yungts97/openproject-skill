use anyhow::{anyhow, bail, Context, Result};
use chrono::Local;
use clap::{Args, Parser, Subcommand};
use keyring::{Entry as KeyringEntry, Error as KeyringError};
use regex::Regex;
use reqwest::blocking::{Client as HttpClient, RequestBuilder};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, LOCATION};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::HashSet;
use std::env;
use std::fs;
use std::io::ErrorKind;
use std::io::{self, IsTerminal, Write};
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::process::Stdio;
use std::process::{Command, ExitCode};
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
    Projects(PageArgs),
    /// List OpenProject work package statuses.
    Statuses(PageArgs),
    /// List OpenProject work package priorities.
    Priorities(PageArgs),
    /// List work package types available in a project.
    Types(ProjectPageArgs),
    /// List users who can be assigned work in a project.
    Users(ProjectPageArgs),
    /// List versions available in a project.
    Versions(ProjectPageArgs),
    /// List work package categories available in a project.
    Categories(ProjectPageArgs),
    /// Resolve the project for this repository.
    Project(ProjectArg),
    /// List work packages in a project.
    Tasks(TasksArgs),
    /// Show one work package.
    Task(TaskArgs),
    /// List activity entries for a work package.
    Activities(ActivityArgs),
    /// Show one activity with its comment and change details.
    Activity(ActivityIdArgs),
    /// List the time-entry activities available for a work package.
    TimeEntryActivities(TimeEntryActivitiesArgs),
    /// List relations for a work package.
    Relations(ActivityArgs),
    /// Create or delete work package relations.
    Relation {
        #[command(subcommand)]
        command: RelationCommands,
    },
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
    /// Remove this OpenProject executable. Configuration and Agent Skill files are preserved unless --purge is used.
    Uninstall(UninstallArgs),
}

#[derive(Subcommand, Debug)]
enum AuthCommands {
    /// Interactively verify and securely save an OpenProject API token.
    Login,
    /// Show whether a credential is available and valid.
    Status,
    /// Remove the saved credential while keeping the configured host.
    Logout,
    Verify,
}

#[derive(Subcommand, Debug)]
enum RelationCommands {
    /// Create a relation from one work package to another.
    Add(RelationAddArgs),
    /// Delete a relation by relation ID.
    Delete(RelationDeleteArgs),
}

#[derive(Args, Debug)]
struct ProjectArg {
    #[arg(long)]
    project: Option<String>,
    /// Persist the resolved project ID in the repository .openproject.json file.
    #[arg(long)]
    bind: bool,
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
    /// Filter by an exact status name or numeric ID. Repeat for multiple values.
    #[arg(long)]
    status: Vec<String>,
    /// Filter by an exact type name or numeric ID. Repeat for multiple values.
    #[arg(long = "type")]
    types: Vec<String>,
    /// Filter by an exact priority name or numeric ID. Repeat for multiple values.
    #[arg(long)]
    priority: Vec<String>,
    /// Include work packages due on or before this YYYY-MM-DD date.
    #[arg(long)]
    due_before: Option<String>,
    /// Include work packages updated within this many days, e.g. 7 or 7d.
    #[arg(long)]
    updated_since: Option<String>,
    /// Sort as FIELD[:asc|desc]. Repeat for secondary sorting.
    #[arg(long)]
    sort: Vec<String>,
    #[command(flatten)]
    page: PageArgs,
}

#[derive(Args, Debug)]
struct PageArgs {
    /// Maximum records to return (1-1000).
    #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u32).range(1..=1000))]
    limit: u32,
    /// One-based result-page offset.
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
    offset: u32,
}

#[derive(Args, Debug)]
struct ProjectPageArgs {
    #[arg(long)]
    project: Option<String>,
    #[command(flatten)]
    page: PageArgs,
}

#[derive(Args, Debug)]
struct TaskArgs {
    task_id: u64,
    /// Return the complete API representation instead of a compact summary.
    #[arg(long)]
    full: bool,
}

#[derive(Args, Debug)]
struct ActivityArgs {
    task_id: u64,
    #[command(flatten)]
    page: PageArgs,
}

#[derive(Args, Debug)]
struct ActivityIdArgs {
    activity_id: u64,
}

#[derive(Args, Debug)]
struct RelationAddArgs {
    /// Work package from which the relation originates.
    from_id: u64,
    /// Work package to which the relation points.
    #[arg(long)]
    to: u64,
    /// Relation type from the perspective of FROM_ID.
    #[arg(
        long,
        default_value = "relates",
        value_parser = [
            "relates", "duplicates", "duplicated", "blocks", "blocked", "precedes",
            "follows", "includes", "partof", "requires", "required"
        ]
    )]
    r#type: String,
    #[arg(long)]
    description: Option<String>,
    /// Lag in days; supported only by applicable relation types.
    #[arg(long)]
    lag: Option<u64>,
}

#[derive(Args, Debug)]
struct RelationDeleteArgs {
    relation_id: u64,
}

#[derive(Args, Debug)]
struct TimeEntryActivitiesArgs {
    task_id: u64,
}

#[derive(Args, Debug)]
struct UpgradeArgs {
    /// Release version to install, or "latest".
    #[arg(default_value = "latest")]
    version: String,
}

#[derive(Args, Debug)]
struct UninstallArgs {
    /// Also remove global configuration and the stored credential for its host.
    #[arg(long)]
    purge: bool,
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
    priority: Option<String>,
    #[arg(long)]
    responsible: Option<String>,
    #[arg(long)]
    parent: Option<u64>,
    #[arg(long)]
    version: Option<String>,
    #[arg(long)]
    start_date: Option<String>,
    #[arg(long)]
    due_date: Option<String>,
    #[arg(long)]
    estimate: Option<String>,
    /// Set a scalar custom field as customFieldN=JSON. Non-JSON values are strings.
    #[arg(long = "custom-field", value_name = "CUSTOM_FIELD=JSON")]
    custom_fields: Vec<String>,
    /// Set a linked custom field as customFieldN=/api/v3/RESOURCE/ID.
    #[arg(long = "custom-field-link", value_name = "CUSTOM_FIELD=HREF")]
    custom_field_links: Vec<String>,
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
    #[arg(long)]
    priority: Option<String>,
    #[arg(long)]
    responsible: Option<String>,
    #[arg(long)]
    parent: Option<u64>,
    #[arg(long)]
    version: Option<String>,
    #[arg(long, value_parser = clap::value_parser!(u8).range(0..=100))]
    percent: Option<u8>,
    #[arg(long)]
    start_date: Option<String>,
    #[arg(long)]
    due_date: Option<String>,
    #[arg(long)]
    estimate: Option<String>,
    /// Set a scalar custom field as customFieldN=JSON. Non-JSON values are strings.
    #[arg(long = "custom-field", value_name = "CUSTOM_FIELD=JSON")]
    custom_fields: Vec<String>,
    /// Set a linked custom field as customFieldN=/api/v3/RESOURCE/ID.
    #[arg(long = "custom-field-link", value_name = "CUSTOM_FIELD=HREF")]
    custom_field_links: Vec<String>,
    /// Clear the work package description.
    #[arg(long)]
    clear_description: bool,
    /// Remove the assignee.
    #[arg(long)]
    clear_assignee: bool,
    /// Remove the responsible user.
    #[arg(long)]
    clear_responsible: bool,
    /// Remove the parent work package.
    #[arg(long)]
    clear_parent: bool,
    /// Remove the assigned version.
    #[arg(long)]
    clear_version: bool,
    /// Clear the start date.
    #[arg(long)]
    clear_start_date: bool,
    /// Clear the due date.
    #[arg(long)]
    clear_due_date: bool,
    /// Clear the estimated time.
    #[arg(long)]
    clear_estimate: bool,
    /// Clear a scalar customFieldN value. Repeat for multiple fields.
    #[arg(long = "clear-custom-field", value_name = "CUSTOM_FIELD")]
    clear_custom_fields: Vec<String>,
    /// Clear a linked customFieldN value. Repeat for multiple fields.
    #[arg(long = "clear-custom-field-link", value_name = "CUSTOM_FIELD")]
    clear_custom_field_links: Vec<String>,
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
    /// Time-entry activity name or numeric ID.
    #[arg(long, visible_alias = "activity-id")]
    activity: Option<String>,
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
        let retries = if method == reqwest::Method::GET { 2 } else { 0 };
        let mut attempt = 0;
        let response = loop {
            let mut request: RequestBuilder = self
                .http
                .request(method.clone(), &url)
                .header(ACCEPT, API_ACCEPT)
                .header(AUTHORIZATION, format!("Bearer {}", self.token));
            if let Some(payload) = &body {
                request = request
                    .header(CONTENT_TYPE, "application/json")
                    .json(payload);
            }
            match request.send() {
                Ok(response)
                    if attempt < retries
                        && (response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS
                            || response.status().is_server_error()) =>
                {
                    let wait = response
                        .headers()
                        .get("retry-after")
                        .and_then(|value| value.to_str().ok())
                        .and_then(|value| value.parse::<u64>().ok())
                        .unwrap_or(1)
                        .min(10);
                    std::thread::sleep(std::time::Duration::from_secs(wait));
                    attempt += 1;
                }
                Ok(response) => break response,
                Err(error) if attempt < retries => {
                    attempt += 1;
                    std::thread::sleep(std::time::Duration::from_secs(attempt));
                    if attempt > retries {
                        return Err(error).context("cannot connect to OpenProject");
                    }
                }
                Err(error) => return Err(error).context("cannot connect to OpenProject"),
            }
        };
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
    fn page(&self, path: &str, page: &PageArgs) -> Result<Value> {
        let query = format!("pageSize={}&offset={}", page.limit, page.offset);
        self.get(&format!(
            "{path}{}{}",
            if path.contains('?') { "&" } else { "?" },
            query
        ))
    }
}

#[derive(Debug, Default, PartialEq)]
struct Config {
    host: Option<String>,
    project: Option<String>,
    credential_store: Option<CredentialStoreKind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CredentialStoreKind {
    Native,
    File,
}

impl CredentialStoreKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::File => "file",
        }
    }
}

fn global_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|path| path.join("openproject").join("config.json"))
}

fn write_global_config(host: &str, credential_store: Option<CredentialStoreKind>) -> Result<()> {
    let path = global_config_path()
        .ok_or_else(|| anyhow!("cannot determine the global config directory"))?;
    let mut settings = read_config(&path)?;
    validate_keys(&settings, &["host", "credential_store"], &path)?;
    settings.insert("host".into(), Value::String(host.to_owned()));
    match credential_store {
        Some(kind) => {
            settings.insert(
                "credential_store".into(),
                Value::String(kind.as_str().to_owned()),
            );
        }
        None => {
            settings.remove("credential_store");
        }
    }
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("global config path has no parent directory"))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("cannot create global config directory {}", parent.display()))?;
    let contents = serde_json::to_string_pretty(&Value::Object(settings))?;
    fs::write(&path, format!("{contents}\n"))
        .with_context(|| format!("cannot write {}", path.display()))
}

fn credential_store_setting(
    settings: &Map<String, Value>,
    path: &Path,
) -> Result<Option<CredentialStoreKind>> {
    settings
        .get("credential_store")
        .map(|value| match value.as_str() {
            Some("native") => Ok(CredentialStoreKind::Native),
            Some("file") => Ok(CredentialStoreKind::File),
            _ => bail!(
                "credential_store in {} must be \"native\" or \"file\"",
                path.display()
            ),
        })
        .transpose()
}

fn project_config_path(cwd: &Path) -> PathBuf {
    git_root(cwd)
        .unwrap_or_else(|| cwd.to_path_buf())
        .join(".openproject.json")
}

fn bind_project(cwd: &Path, project_id: u64) -> Result<PathBuf> {
    let path = project_config_path(cwd);
    let mut settings = read_config(&path)?;
    validate_keys(&settings, &["host", "project_id", "project"], &path)?;
    settings.insert("project_id".into(), Value::Number(project_id.into()));
    settings.remove("project");
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("project config path has no parent directory"))?;
    fs::create_dir_all(parent)
        .with_context(|| format!("cannot create repository directory {}", parent.display()))?;
    let temporary = parent.join(format!(".openproject-{}.tmp", std::process::id()));
    let result = fs::write(
        &temporary,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&Value::Object(settings))?
        ),
    )
    .and_then(|_| fs::rename(&temporary, &path));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.with_context(|| format!("cannot write {}", path.display()))?;
    Ok(path)
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
    validate_keys(global, &["host", "credential_store"], global_path)?;
    validate_keys(project, &["host", "project_id", "project"], project_path)?;
    Ok(Config {
        host: host_setting(project, project_path)?.or(host_setting(global, global_path)?),
        project: project_setting(project, project_path)?,
        credential_store: credential_store_setting(global, global_path)?,
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
    fn delete(&self) -> Result<bool>;
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
        if !Self::available() {
            bail!("system credential store is unavailable; check access to your desktop credential service from this session")
        }
        let entry = self
            .entry()
            .map_err(|_| anyhow!("cannot access the system credential store"))?;
        match entry.get_password() {
            Ok(token) => Ok(Some(token)),
            Err(KeyringError::NoEntry) => Ok(None),
            Err(_) => bail!("cannot read the stored OpenProject token; check that the system credential store is unlocked and accessible from this session"),
        }
    }

    fn save(&self, token: &str) -> Result<()> {
        self.entry()?
            .set_password(token)
            .context("cannot save the OpenProject token in the system credential store")
    }

    fn delete(&self) -> Result<bool> {
        match self.entry()?.delete_credential() {
            Ok(()) => Ok(true),
            Err(KeyringError::NoEntry) => Ok(false),
            Err(error) => Err(anyhow!(error))
                .context("cannot remove the OpenProject token from the system credential store"),
        }
    }
}

#[derive(Serialize, Deserialize, Default)]
struct CredentialFile {
    version: u8,
    credentials: std::collections::BTreeMap<String, CredentialRecord>,
}
#[derive(Serialize, Deserialize)]
struct CredentialRecord {
    token: String,
}

struct FileCredentialStore {
    host: String,
    path: PathBuf,
}
impl FileCredentialStore {
    fn new(host: &str) -> Result<Self> {
        let path = dirs::config_dir()
            .ok_or_else(|| anyhow!("cannot determine the OpenProject config directory"))?
            .join("openproject")
            .join("credentials.json");
        Ok(Self {
            host: canonical_host(host)?,
            path,
        })
    }
    #[cfg(test)]
    fn at(host: &str, path: PathBuf) -> Self {
        Self {
            host: canonical_host(host).unwrap(),
            path,
        }
    }
    fn read(&self) -> Result<CredentialFile> {
        match fs::read_to_string(&self.path) {
            Ok(raw) => serde_json::from_str(&raw)
                .map_err(|_| anyhow!("cannot read the OpenProject credential file")),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(CredentialFile {
                version: 1,
                ..Default::default()
            }),
            Err(_) => bail!("cannot read the OpenProject credential file"),
        }
    }
    fn write(&self, value: &CredentialFile) -> Result<()> {
        let directory = self
            .path
            .parent()
            .ok_or_else(|| anyhow!("credential path has no directory"))?;
        fs::create_dir_all(directory)
            .context("cannot create the OpenProject credential directory")?;
        #[cfg(unix)]
        {
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
                .context("cannot protect the OpenProject credential directory")?;
        }
        let temp = directory.join(format!(
            ".credentials-{}-{}.tmp",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            options.mode(0o600);
        }
        let mut file = options
            .open(&temp)
            .context("cannot create protected temporary credential file")?;
        let result = (|| -> Result<()> {
            file.write_all(serde_json::to_string_pretty(value)?.as_bytes())?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            fs::rename(&temp, &self.path)
                .context("cannot atomically replace the OpenProject credential file")?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temp);
        }
        result
    }
}
impl CredentialStore for FileCredentialStore {
    fn name(&self) -> &'static str {
        "protected credential file"
    }
    fn load(&self) -> Result<Option<String>> {
        Ok(self
            .read()?
            .credentials
            .remove(&self.host)
            .map(|record| record.token))
    }
    fn save(&self, token: &str) -> Result<()> {
        let mut file = self.read()?;
        file.version = 1;
        file.credentials.insert(
            self.host.clone(),
            CredentialRecord {
                token: token.to_owned(),
            },
        );
        self.write(&file)
    }
    fn delete(&self) -> Result<bool> {
        let mut file = self.read()?;
        let existed = file.credentials.remove(&self.host).is_some();
        if existed {
            if file.credentials.is_empty() {
                match fs::remove_file(&self.path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(error).with_context(|| {
                            format!(
                                "cannot remove empty OpenProject credential file {}",
                                self.path.display()
                            )
                        })
                    }
                }
            } else {
                self.write(&file)?;
            }
        }
        Ok(existed)
    }
}

struct ResolvedCredential {
    token: String,
    source: &'static str,
}
fn store_for(kind: CredentialStoreKind, host: &str) -> Result<Box<dyn CredentialStore>> {
    match kind {
        CredentialStoreKind::Native => Ok(Box::new(NativeCredentialStore::new(credential_scope(
            host,
        )?))),
        CredentialStoreKind::File => Ok(Box::new(FileCredentialStore::new(host)?)),
    }
}
fn resolve_credential(host: &str, config: &Config) -> Result<ResolvedCredential> {
    if let Ok(token) = env::var("OPENPROJECT_TOKEN") {
        return Ok(ResolvedCredential {
            token,
            source: "environment",
        });
    }
    if let Some(path) = env::var_os("OPENPROJECT_TOKEN_FILE") {
        let token = fs::read_to_string(path)
            .map_err(|_| anyhow!("OPENPROJECT_TOKEN_FILE could not be read"))?;
        let token = token.trim().to_owned();
        if token.is_empty() {
            bail!("OPENPROJECT_TOKEN_FILE is empty");
        }
        return Ok(ResolvedCredential {
            token,
            source: "token_file",
        });
    }
    let kinds: Vec<CredentialStoreKind> = match config.credential_store {
        Some(CredentialStoreKind::Native) => {
            vec![CredentialStoreKind::Native, CredentialStoreKind::File]
        }
        Some(CredentialStoreKind::File) => vec![CredentialStoreKind::File],
        None => vec![CredentialStoreKind::Native, CredentialStoreKind::File],
    };
    let mut configured_error = None;
    for kind in kinds {
        match store_for(kind, host)?.load() {
            Ok(Some(token)) => {
                return Ok(ResolvedCredential {
                    token,
                    source: kind.as_str(),
                })
            }
            Ok(None) => (),
            Err(error) if config.credential_store == Some(kind) => configured_error = Some(error),
            Err(_) => (),
        }
    }
    if configured_error.is_some() {
        bail!("The configured system credential could not be read.\n\nRun:\n  openproject auth login\n\nor use OPENPROJECT_TOKEN / OPENPROJECT_TOKEN_FILE.");
    }
    bail!("No OpenProject credential found.\n\nRun:\n  openproject auth login\n\nFor CI or containers, set:\n  OPENPROJECT_TOKEN\nor:\n  OPENPROJECT_TOKEN_FILE")
}
fn resolve_token(host: &str, config: &Config) -> Result<String> {
    Ok(resolve_credential(host, config)?.token)
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

fn auth_login(cli: &Cli, cfg: &Config) -> Result<()> {
    if cli.json {
        bail!("auth login cannot be used with --json")
    }
    require_interactive_terminal()?;
    println!("OpenProject CLI setup\n");
    println!("[1/3] OpenProject server");
    let entered_host = match cli.host.as_deref().or(cfg.host.as_deref()) {
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

    let existing = resolve_credential(&host, cfg).ok();
    if existing.is_some()
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

    let preferred = if env::var_os("WSL_DISTRO_NAME").is_some()
        || (cfg!(target_os = "linux") && env::var_os("DBUS_SESSION_BUS_ADDRESS").is_none())
    {
        CredentialStoreKind::File
    } else {
        CredentialStoreKind::Native
    };
    let mut saved = None;
    for kind in [preferred, CredentialStoreKind::File] {
        if saved.is_some() {
            continue;
        }
        let store = store_for(kind, &host)?;
        if store.save(&token).is_ok()
            && store.load().ok().flatten().as_deref() == Some(token.as_str())
        {
            saved = Some((kind, store.name()));
        }
    }
    let Some((kind, saved_by)) = saved else {
        bail!("the token was verified but could not be saved; retry auth login or use OPENPROJECT_TOKEN / OPENPROJECT_TOKEN_FILE")
    };
    write_global_config(&host, Some(kind))?;
    println!("\nSetup complete. Host saved to global configuration; token saved in {saved_by}.");
    Ok(())
}

fn auth_status(cli: &Cli, cfg: &Config, host: &str) -> Result<()> {
    let credential = match resolve_credential(host, cfg) {
        Ok(credential) => credential,
        Err(_) => {
            if cli.json {
                emit(
                    json!({"host":host,"authenticated":false,"credential_source":null,"user":null}),
                    true,
                );
            } else {
                println!("OpenProject authentication\n\nHost:       {host}\nCredential: unavailable\nStatus:     not authenticated");
            }
            return Ok(());
        }
    };
    let client = OpenProjectClient::new(host.to_owned(), credential.token)?;
    let user = client.get("/users/me");
    let authenticated = user.is_ok();
    let user = user.ok().map(|value| json!({"id":value.get("id"),"name":value.get("name").or_else(|| value.get("login"))}));
    if cli.json {
        emit(
            json!({"host":host,"authenticated":authenticated,"credential_source":credential.source,"user":user}),
            true,
        );
        return Ok(());
    }
    println!("OpenProject authentication\n\nHost:       {host}\nCredential: available\nStorage:    {}\nStatus:     {}", credential.source.replace('_', " "), if authenticated { "authenticated" } else { "authentication failed" });
    Ok(())
}

fn auth_logout(cli: &Cli, cfg: &Config, host: &str) -> Result<()> {
    let kind = cfg
        .credential_store
        .ok_or_else(|| anyhow!("no persistent credential store is configured"))?;
    let removed = store_for(kind, host)?.delete()?;
    write_global_config(host, None)?;
    emit(json!({"host":host,"credentialRemoved":removed}), cli.json);
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

fn add_project_candidate(candidates: &mut Vec<String>, value: &str) {
    let value = value.trim();
    if !value.is_empty()
        && !normalize(value).is_empty()
        && !candidates
            .iter()
            .any(|candidate| normalize(candidate) == normalize(value))
    {
        candidates.push(value.to_owned());
    }
}

fn manifest_name(contents: &str, section: &str) -> Option<String> {
    let mut in_section = false;
    for line in contents.lines() {
        let line = line.trim();
        if line.starts_with('[') && line.ends_with(']') {
            in_section = line == format!("[{section}]");
            continue;
        }
        if in_section {
            if let Some((key, value)) = line.split_once('=') {
                if key.trim() == "name" {
                    return Some(value.trim().trim_matches(['\"', '\'']).to_owned());
                }
            }
        }
    }
    None
}

fn project_candidates(cwd: &Path) -> Vec<String> {
    let root = git_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    let mut candidates = Vec::new();
    if let Some(name) = root.file_name().and_then(|name| name.to_str()) {
        add_project_candidate(&mut candidates, name);
    }
    if root != cwd {
        if let Some(name) = cwd.file_name().and_then(|name| name.to_str()) {
            add_project_candidate(&mut candidates, name);
        }
    }

    for readme in ["README.md", "README.MD", "readme.md", "README"] {
        let path = root.join(readme);
        if let Ok(contents) = fs::read_to_string(path) {
            if let Some(heading) = contents
                .lines()
                .find_map(|line| line.trim().strip_prefix("# "))
            {
                add_project_candidate(&mut candidates, heading);
            }
            break;
        }
    }

    for (file, section) in [("Cargo.toml", "package"), ("pyproject.toml", "project")] {
        if let Ok(contents) = fs::read_to_string(root.join(file)) {
            if let Some(name) = manifest_name(&contents, section) {
                add_project_candidate(&mut candidates, &name);
            }
        }
    }
    for file in ["package.json", "composer.json"] {
        if let Ok(contents) = fs::read_to_string(root.join(file)) {
            if let Some(name) = serde_json::from_str::<Value>(&contents)
                .ok()
                .and_then(|value| value.get("name").and_then(Value::as_str).map(str::to_owned))
            {
                add_project_candidate(&mut candidates, &name);
            }
        }
    }
    candidates
}

fn project_relevance(candidate: &str, project: &str) -> Option<usize> {
    let candidate = normalize(candidate);
    let project = normalize(project);
    if candidate.is_empty() || project.is_empty() || candidate == project {
        return None;
    }
    let shortest = candidate.len().min(project.len());
    if shortest >= 4 && (candidate.contains(&project) || project.contains(&candidate)) {
        return Some(200 + shortest);
    }

    let ignored = [
        "app", "api", "backend", "cli", "client", "core", "frontend", "service", "web",
    ];
    let candidate_tokens: HashSet<_> = candidate
        .split_whitespace()
        .filter(|token| token.len() >= 3 && !ignored.contains(token))
        .collect();
    let project_tokens: HashSet<_> = project
        .split_whitespace()
        .filter(|token| token.len() >= 3 && !ignored.contains(token))
        .collect();
    let shared = candidate_tokens.intersection(&project_tokens).count();
    if shared >= 2
        || (shared == 1 && candidate_tokens.len().min(project_tokens.len()) == 1 && shortest >= 4)
    {
        Some(100 + shared * 10)
    } else {
        None
    }
}

fn related_projects(projects: &[Value], candidates: &[String]) -> Vec<String> {
    let mut matches: Vec<_> = projects
        .iter()
        .filter_map(|project| {
            let name = project.get("name").and_then(Value::as_str)?;
            let identifier = project.get("identifier").and_then(Value::as_str);
            let score = candidates
                .iter()
                .filter_map(|candidate| {
                    project_relevance(candidate, name)
                        .into_iter()
                        .chain(
                            identifier
                                .and_then(|identifier| project_relevance(candidate, identifier)),
                        )
                        .max()
                })
                .max()?;
            let label = match (project.get("id").and_then(Value::as_u64), identifier) {
                (Some(id), Some(identifier)) => format!("{name} ({identifier}, ID {id})"),
                (Some(id), None) => format!("{name} (ID {id})"),
                (None, Some(identifier)) => format!("{name} ({identifier})"),
                (None, None) => name.to_owned(),
            };
            Some((score, label))
        })
        .collect();
    matches.sort_by(|(left_score, left_label), (right_score, right_label)| {
        right_score
            .cmp(left_score)
            .then_with(|| left_label.cmp(right_label))
    });
    matches
        .into_iter()
        .take(5)
        .map(|(_, label)| label)
        .collect()
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
    if let Some(value) = value {
        if let Ok(number) = value.parse::<u64>() {
            return client.get(&format!("/projects/{number}"));
        }
        let projects = client.collection("/projects")?;
        let normalized_value = normalize(&value);
        let matches: Vec<_> = projects
            .into_iter()
            .filter(|p| {
                [p.get("name"), p.get("identifier")]
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .any(|name| normalize(name) == normalized_value)
            })
            .collect();
        return match matches.len() {
            1 => Ok(matches.into_iter().next().unwrap()),
            0 => bail!("no OpenProject project exactly matches {value:?}"),
            _ => bail!("multiple OpenProject projects match {value:?}; use a numeric --project"),
        };
    }
    let projects = client.collection("/projects")?;
    let candidates = project_candidates(cwd);
    let normalized_candidates: HashSet<_> = candidates
        .iter()
        .map(|candidate| normalize(candidate))
        .collect();
    let matches: Vec<_> = projects
        .iter()
        .filter(|p| {
            [p.get("name"), p.get("identifier")]
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .any(|project_name| normalized_candidates.contains(&normalize(project_name)))
        })
        .cloned()
        .collect();
    match matches.len() {
        1 => Ok(matches.into_iter().next().unwrap()),
        0 => {
            let related = related_projects(&projects, &candidates);
            if related.is_empty() {
                bail!("cannot resolve this directory to one project safely; use --project or .openproject.json");
            }
            bail!(
                "no exact project match for this directory; related projects: {}. Select one with --project or .openproject.json",
                related.join("; ")
            )
        }
        _ => bail!(
            "multiple OpenProject projects exactly match this directory; use a numeric --project"
        ),
    }
}

fn resolve_item(client: &OpenProjectClient, path: &str, value: &str, kind: &str) -> Result<u64> {
    if let Ok(n) = value.parse() {
        return Ok(n);
    }
    let normalized_value = normalize(value);
    let matches: Vec<_> = client
        .collection(path)?
        .into_iter()
        .filter(|item| {
            item.get("name")
                .and_then(Value::as_str)
                .map(|name| normalize(name) == normalized_value)
                .unwrap_or(false)
        })
        .collect();
    match matches.len() {
        1 => id(&matches[0]),
        0 => bail!("no {kind} exactly matches {value:?}"),
        _ => bail!("multiple {kind} values match {value:?}; use a numeric ID"),
    }
}

fn resolve_items(
    client: &OpenProjectClient,
    path: &str,
    values: &[String],
    kind: &str,
) -> Result<Vec<u64>> {
    if values.iter().all(|value| value.parse::<u64>().is_ok()) {
        return values
            .iter()
            .map(|value| value.parse::<u64>().map_err(Into::into))
            .collect();
    }
    let items = client.collection(path)?;
    values
        .iter()
        .map(|value| {
            if let Ok(number) = value.parse() {
                return Ok(number);
            }
            let normalized_value = normalize(value);
            let matches = items
                .iter()
                .filter(|item| {
                    item.get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|name| normalize(name) == normalized_value)
                })
                .collect::<Vec<_>>();
            match matches.as_slice() {
                [item] => id(item),
                [] => bail!("no {kind} exactly matches {value:?}"),
                _ => bail!("multiple {kind} values match {value:?}; use a numeric ID"),
            }
        })
        .collect()
}

fn custom_field_key(value: &str) -> Result<String> {
    let Some(number) = value.strip_prefix("customField") else {
        bail!("custom field keys must use the OpenProject customFieldN property name")
    };
    if number.is_empty()
        || !number.chars().all(|character| character.is_ascii_digit())
        || number
            .parse::<u64>()
            .ok()
            .filter(|number| *number > 0)
            .is_none()
    {
        bail!("custom field keys must use the OpenProject customFieldN property name")
    }
    Ok(value.to_owned())
}

fn custom_field_operations(
    scalar_values: &[String],
    linked_values: &[String],
    clear_scalar: &[String],
    clear_linked: &[String],
) -> Result<(Map<String, Value>, Map<String, Value>)> {
    let mut properties = Map::new();
    let mut links = Map::new();
    let mut seen = HashSet::new();

    for assignment in scalar_values {
        let (key, raw) = assignment
            .split_once('=')
            .ok_or_else(|| anyhow!("custom fields must use customFieldN=JSON"))?;
        let key = custom_field_key(key)?;
        if !seen.insert(key.clone()) {
            bail!("custom field {key} was supplied more than once");
        }
        let value = serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_owned()));
        properties.insert(key, value);
    }
    for assignment in linked_values {
        let (key, href) = assignment
            .split_once('=')
            .ok_or_else(|| anyhow!("linked custom fields must use customFieldN=HREF"))?;
        let key = custom_field_key(key)?;
        if !seen.insert(key.clone()) {
            bail!("custom field {key} was supplied more than once");
        }
        if !href.starts_with("/api/v3/") {
            bail!("linked custom field hrefs must start with /api/v3/");
        }
        links.insert(key, json!({"href": href}));
    }
    for key in clear_scalar {
        let key = custom_field_key(key)?;
        if !seen.insert(key.clone()) {
            bail!("custom field {key} was supplied more than once");
        }
        properties.insert(key, Value::Null);
    }
    for key in clear_linked {
        let key = custom_field_key(key)?;
        if !seen.insert(key.clone()) {
            bail!("custom field {key} was supplied more than once");
        }
        links.insert(key, Value::Null);
    }
    Ok((properties, links))
}

fn updated_since_days(value: &str) -> Result<u32> {
    let value = value.strip_suffix('d').unwrap_or(value);
    let days = value
        .parse::<u32>()
        .context("--updated-since must be a positive day count such as 7 or 7d")?;
    if days == 0 {
        bail!("--updated-since must be at least one day");
    }
    Ok(days)
}

fn sort_criteria(values: &[String]) -> Result<Vec<Value>> {
    values
        .iter()
        .map(|value| {
            let (field, direction) = value.split_once(':').unwrap_or((value, "asc"));
            let field = match field.to_ascii_lowercase().replace('_', "-").as_str() {
                "id" => "id",
                "type" => "type",
                "status" => "status",
                "priority" => "priority",
                "subject" => "subject",
                "assignee" => "assignee",
                "responsible" => "responsible",
                "start" | "start-date" => "startDate",
                "due" | "due-date" => "dueDate",
                "created" | "created-at" => "createdAt",
                "updated" | "updated-at" => "updatedAt",
                "percent" | "percentage-done" => "percentageDone",
                "estimate" | "estimated-time" => "estimatedTime",
                _ => bail!(
                    "unsupported sort field {field:?}; use id, type, status, priority, subject, assignee, responsible, start-date, due-date, created-at, updated-at, percentage-done, or estimated-time"
                ),
            };
            let direction = direction.to_ascii_lowercase();
            if direction != "asc" && direction != "desc" {
                bail!("sort direction must be asc or desc");
            }
            Ok(json!([field, direction]))
        })
        .collect()
}

fn time_entry_activity_values(form: &Value) -> Result<Vec<Value>> {
    let field = form
        .pointer("/_embedded/schema/activity")
        .ok_or_else(|| anyhow!("OpenProject time-entry form has no activity schema"))?;
    let values = field
        .pointer("/_embedded/allowedValues")
        .or_else(|| field.pointer("/_links/allowedValues"))
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("OpenProject time-entry form has no allowed activity values"))?;
    values
        .iter()
        .map(|value| {
            let href = value
                .pointer("/_links/self/href")
                .or_else(|| value.get("href"))
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("time-entry activity has no link"))?;
            let name = value
                .get("name")
                .or_else(|| value.get("title"))
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("time-entry activity has no name"))?;
            let activity_id = value.get("id").and_then(Value::as_u64).or_else(|| {
                href.split('?')
                    .next()
                    .and_then(|path| path.rsplit('/').next())
                    .and_then(|part| part.parse().ok())
            });
            Ok(json!({"id": activity_id, "name": name, "href": href}))
        })
        .collect()
}
fn time_entry_activity_form(client: &OpenProjectClient, task_id: u64) -> Result<Vec<Value>> {
    let task = client.get(&format!("/work_packages/{task_id}"))?;
    let project = href(&task, "project")
        .ok_or_else(|| anyhow!("work package response has no project link"))?;
    let work_package = format!("/api/v3/work_packages/{task_id}");
    let form = client.request(
        reqwest::Method::POST,
        "/time_entries/form",
        Some(json!({"_links": {
            "entity": {"href": work_package},
            "workPackage": {"href": work_package},
            "project": {"href": project}
        }})),
    )?;
    time_entry_activity_values(&form)
}
fn time_entry_activities(client: &OpenProjectClient, task_id: u64) -> Result<Value> {
    Ok(json!({
        "taskId": task_id,
        "activities": time_entry_activity_form(client, task_id)?
    }))
}
fn resolve_time_entry_activity(
    client: &OpenProjectClient,
    task_id: u64,
    value: &str,
) -> Result<String> {
    let activities = time_entry_activity_form(client, task_id)?;
    let normalized_value = normalize(value);
    let matches = activities
        .iter()
        .filter(|activity| {
            activity.get("id").and_then(Value::as_u64) == value.parse().ok()
                || activity
                    .get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| normalize(name) == normalized_value)
        })
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [activity] => activity
            .get("href")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("time-entry activity has no link")),
        [] => bail!("no available time-entry activity exactly matches {value:?}"),
        _ => bail!("multiple time-entry activities match {value:?}; use a numeric ID"),
    }
}
fn resolve_user(client: &OpenProjectClient, value: &str) -> Result<u64> {
    if value.eq_ignore_ascii_case("me") {
        return id(&client.get("/users/me")?);
    }
    value
        .parse()
        .map_err(|_| anyhow!("user must be a numeric ID or 'me'"))
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

fn page_elements(page: &Value, host: &str) -> Value {
    let items = page
        .pointer("/_embedded/elements")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|item| task_summary(item, host))
        .collect::<Vec<_>>();
    json!({
        "items": items,
        "total": page.get("total"),
        "count": page.get("count"),
        "offset": page.get("offset"),
        "pageSize": page.get("pageSize"),
        "next": page.pointer("/_links/nextByOffset/href").and_then(Value::as_str),
    })
}

fn activity_page(client: &OpenProjectClient, args: &ActivityArgs) -> Result<Value> {
    let mut page = client.page(
        &format!("/work_packages/{}/activities", args.task_id),
        &args.page,
    )?;
    let activity_entries = page
        .pointer("/_embedded/elements")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .cloned()
        .collect::<Vec<_>>();
    let server_paginates = page.get("pageSize").is_some();
    let total = activity_entries.len();
    let start = if server_paginates {
        0
    } else {
        (args.page.offset.saturating_sub(1) * args.page.limit) as usize
    };
    let selected = activity_entries
        .into_iter()
        .skip(start)
        .take(args.page.limit as usize)
        .collect::<Vec<_>>();
    let details = selected
        .into_iter()
        .map(|activity| client.get(&format!("/activities/{}", id(&activity)?)))
        .collect::<Result<Vec<_>>>()?;
    if let Some(elements) = page
        .pointer_mut("/_embedded/elements")
        .and_then(Value::as_array_mut)
    {
        *elements = details;
    }
    if !server_paginates {
        let count = page
            .pointer("/_embedded/elements")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        let page_object = page
            .as_object_mut()
            .expect("OpenProject activity collection response is an object");
        page_object.insert("total".into(), json!(total));
        page_object.insert("count".into(), json!(count));
        page_object.insert("offset".into(), json!(args.page.offset));
        page_object.insert("pageSize".into(), json!(args.page.limit));
    }
    Ok(page)
}

fn relation_path(task_id: u64) -> Result<String> {
    let filters = json!([{"involved":{"operator":"=","values":[task_id]}}]);
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    serializer.append_pair("filters", &serde_json::to_string(&filters)?);
    Ok(format!("/relations?{}", serializer.finish()))
}

fn hierarchy_relations(task: &Value) -> Vec<Value> {
    let Some(work_package) = task.pointer("/_links/self").cloned() else {
        return Vec::new();
    };
    let mut relations = Vec::new();
    if let Some(parent) = task.pointer("/_links/parent").filter(|link| {
        link.get("href")
            .and_then(Value::as_str)
            .is_some_and(|href| !href.is_empty())
    }) {
        relations.push(json!({
            "_type": "HierarchyRelation",
            "name": "part of",
            "type": "partof",
            "reverseType": "includes",
            "_links": {"from": work_package, "to": parent}
        }));
    }
    for child in task
        .pointer("/_links/children")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        relations.push(json!({
            "_type": "HierarchyRelation",
            "name": "includes",
            "type": "includes",
            "reverseType": "partof",
            "_links": {"from": work_package, "to": child}
        }));
    }
    relations
}

fn relation_page(client: &OpenProjectClient, args: &ActivityArgs) -> Result<Value> {
    let mut page = client.page(&relation_path(args.task_id)?, &args.page)?;
    let hierarchy = hierarchy_relations(&client.get(&format!("/work_packages/{}", args.task_id))?);
    if !hierarchy.is_empty() {
        page.as_object_mut()
            .expect("OpenProject collection response is an object")
            .insert("hierarchy".into(), Value::Array(hierarchy));
    }
    Ok(page)
}

fn work_package_path(
    project_id: u64,
    args: &TasksArgs,
    assignee: Option<u64>,
    statuses: &[u64],
    types: &[u64],
    priorities: &[u64],
) -> Result<String> {
    let mut filters = Vec::new();
    if statuses.is_empty() && !args.all {
        filters.push(json!({"status":{"operator":"o","values":[]}}));
    }
    if !statuses.is_empty() {
        filters.push(json!({"status":{"operator":"=","values":statuses}}));
    }
    if let Some(assignee) = assignee {
        filters.push(json!({"assignee":{"operator":"=","values":[assignee]}}));
    }
    if !types.is_empty() {
        filters.push(json!({"type":{"operator":"=","values":types}}));
    }
    if !priorities.is_empty() {
        filters.push(json!({"priority":{"operator":"=","values":priorities}}));
    }
    if let Some(query) = &args.query {
        filters.push(json!({"subject":{"operator":"~","values":[query]}}));
    }
    if let Some(date) = &args.due_before {
        chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
            .context("--due-before must be YYYY-MM-DD")?;
        filters.push(json!({"dueDate":{"operator":"<=d","values":[date]}}));
    }
    if let Some(value) = &args.updated_since {
        filters.push(json!({"updatedAt":{"operator":">t-","values":[updated_since_days(value)?]}}));
    }
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    if !filters.is_empty() {
        serializer.append_pair("filters", &serde_json::to_string(&filters)?);
    }
    if !args.sort.is_empty() {
        serializer.append_pair(
            "sortBy",
            &serde_json::to_string(&sort_criteria(&args.sort)?)?,
        );
    }
    let query = serializer.finish();
    Ok(if query.is_empty() {
        format!("/projects/{project_id}/work_packages")
    } else {
        format!("/projects/{project_id}/work_packages?{query}")
    })
}
fn item_label(item: &Value) -> Option<&str> {
    item.get("subject")
        .or_else(|| item.get("name"))
        .or_else(|| item.get("title"))
        .or_else(|| item.pointer("/_links/self/title"))
        .or_else(|| item.pointer("/comment/raw"))
        .and_then(Value::as_str)
}

fn emit_items(items: &[Value]) {
    for item in items {
        let id = item.get("id").and_then(Value::as_u64);
        let label = item_label(item).unwrap_or("(untitled)");
        match id {
            Some(id) => println!("#{id} {label}"),
            None => println!("{label}"),
        }
    }
}

fn emit(value: Value, as_json: bool) {
    if as_json {
        println!("{}", serde_json::to_string_pretty(&value).unwrap());
        return;
    }
    if let Some(items) = value.as_array() {
        emit_items(items);
        return;
    }
    if let Some(items) = value.get("items").and_then(Value::as_array) {
        emit_items(items);
        if let Some(next) = value.get("next").and_then(Value::as_str) {
            println!("More results: {next}");
        }
        return;
    }
    if let Some(items) = value
        .pointer("/_embedded/elements")
        .and_then(Value::as_array)
    {
        emit_items(items);
        if let Some(next) = value
            .pointer("/_links/nextByOffset/href")
            .and_then(Value::as_str)
        {
            println!("More results: {next}");
        }
        return;
    }
    if item_label(&value).is_some() {
        emit_items(std::slice::from_ref(&value));
        return;
    }
    println!("{}", serde_json::to_string_pretty(&value).unwrap());
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

fn write_exact(
    client: &OpenProjectClient,
    cli: &Cli,
    method: reqwest::Method,
    path: &str,
    body: Value,
) -> Result<Value> {
    if cli.dry_run {
        Ok(json!({"dryRun":true,"method":method.as_str(),"path":path,"payload":body}))
    } else {
        client.request(method, path, Some(body))
    }
}

fn write_without_body(
    client: &OpenProjectClient,
    cli: &Cli,
    method: reqwest::Method,
    path: &str,
) -> Result<Value> {
    if cli.dry_run {
        Ok(json!({"dryRun":true,"method":method.as_str(),"path":path}))
    } else {
        client.request(method, path, None)
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

struct PurgePlan {
    config_path: PathBuf,
    config_directory: PathBuf,
    credential_host: Option<String>,
}

fn purge_plan(cli: &Cli) -> Result<PurgePlan> {
    let config_path = global_config_path()
        .ok_or_else(|| anyhow!("cannot determine the global config directory"))?;
    let config_directory = config_path
        .parent()
        .ok_or_else(|| anyhow!("global config path has no parent directory"))?
        .to_path_buf();

    let environment_host = env::var("OPENPROJECT_URL").ok();
    let configured_host = if cli.host.is_none() && environment_host.is_none() {
        let settings = read_config(&config_path)?;
        validate_keys(&settings, &["host", "credential_store"], &config_path)?;
        host_setting(&settings, &config_path)?
    } else {
        None
    };
    let credential_host = cli
        .host
        .clone()
        .or(environment_host)
        .or(configured_host)
        .map(|host| canonical_host(&host))
        .transpose()?;

    Ok(PurgePlan {
        config_path,
        config_directory,
        credential_host,
    })
}

fn remove_global_config(plan: &PurgePlan) -> Result<Value> {
    let mut removed_credentials = Vec::new();
    if let Some(host) = &plan.credential_host {
        for kind in [CredentialStoreKind::Native, CredentialStoreKind::File] {
            if kind == CredentialStoreKind::Native && !NativeCredentialStore::available() {
                continue;
            }
            let Ok(store) = store_for(kind, host) else {
                continue;
            };
            if store
                .delete()
                .with_context(|| format!("cannot remove credentials from {}", store.name()))?
            {
                removed_credentials.push(store.name());
            }
        }
    }

    let config_status = match fs::remove_file(&plan.config_path) {
        Ok(()) => "removed",
        Err(error) if error.kind() == ErrorKind::NotFound => "not_found",
        Err(error) => {
            return Err(error).with_context(|| {
                format!("cannot remove global config {}", plan.config_path.display())
            })
        }
    };
    let directory_status = match fs::remove_dir(&plan.config_directory) {
        Ok(()) => "removed",
        Err(error) if error.kind() == ErrorKind::NotFound => "not_found",
        Err(error) if error.kind() == ErrorKind::DirectoryNotEmpty => "not_empty",
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "cannot remove global config directory {}",
                    plan.config_directory.display()
                )
            })
        }
    };

    Ok(json!({
        "configPath": plan.config_path.display().to_string(),
        "configStatus": config_status,
        "configDirectory": plan.config_directory.display().to_string(),
        "configDirectoryStatus": directory_status,
        "credentialHost": &plan.credential_host,
        "credentialStoresRemoved": removed_credentials,
    }))
}

fn installer_url() -> String {
    let repository = release_repository();
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

fn release_repository() -> String {
    env::var("OPENPROJECT_RELEASE_REPOSITORY")
        .unwrap_or_else(|_| DEFAULT_RELEASE_REPOSITORY.to_string())
        .trim_matches('/')
        .to_string()
}

fn latest_release_version() -> Result<String> {
    let repository = release_repository();
    let url = format!("https://github.com/{repository}/releases/latest");
    let client = HttpClient::builder()
        .user_agent(format!("openproject/{}", env!("CARGO_PKG_VERSION")))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("cannot create release version client")?;
    let response = client
        .get(&url)
        .send()
        .with_context(|| format!("cannot check the latest release from {url}"))?;
    if !response.status().is_redirection() {
        bail!(
            "latest release check expected a redirect from {url}, got {}",
            response.status()
        );
    }
    let location = response
        .headers()
        .get(LOCATION)
        .context("latest release redirect did not include a location")?
        .to_str()
        .context("latest release redirect location was invalid")?;
    let release_url = Url::parse(location)
        .with_context(|| format!("latest release redirect location was invalid: {location}"))?;
    let version = release_url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|segment| !segment.is_empty())
        .context("latest release redirect did not include a version")?;
    Ok(version.trim_start_matches('v').to_string())
}

fn requested_release_version(version: &str) -> Result<String> {
    if version.eq_ignore_ascii_case("latest") {
        latest_release_version()
    } else {
        Ok(version.trim_start_matches('v').to_string())
    }
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

    let target_version = requested_release_version(&args.version)?;
    if target_version == env!("CARGO_PKG_VERSION") {
        if cli.json {
            emit(
                json!({
                    "operation":"upgrade",
                    "path":path,
                    "status":"already-current",
                    "version":target_version
                }),
                true,
            );
        } else {
            println!("OpenProject {target_version} is already installed; no upgrade needed.");
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
        schedule_upgrade_installer(cli, &installer, &target_version, destination)
    }
    #[cfg(not(windows))]
    {
        let result = run_upgrade_installer(cli, &installer, &target_version, destination);
        let _ = fs::remove_file(&installer);
        result
    }
}

fn uninstall(cli: &Cli, args: &UninstallArgs) -> Result<()> {
    let executable = env::current_exe().context("cannot locate the current executable")?;
    let path = executable.display().to_string();
    let purge = args.purge.then(|| purge_plan(cli)).transpose()?;
    if cli.dry_run {
        if cli.json {
            emit(
                json!({
                    "dryRun":true,
                    "operation":"uninstall",
                    "path":path,
                    "purge": purge.as_ref().map(|plan| json!({
                        "configPath": plan.config_path.display().to_string(),
                        "configDirectory": plan.config_directory.display().to_string(),
                        "credentialHost": &plan.credential_host,
                    })),
                }),
                true,
            );
        } else {
            println!("Would remove {path}");
            if let Some(plan) = &purge {
                println!("Would remove global config {}", plan.config_path.display());
                println!(
                    "Would remove global config directory {} if it is empty",
                    plan.config_directory.display()
                );
                if let Some(host) = &plan.credential_host {
                    println!("Would remove stored credentials for {host}");
                } else {
                    println!("No configured host found; stored credentials would be preserved");
                }
            }
        }
        return Ok(());
    }

    let purge = purge.as_ref().map(remove_global_config).transpose()?;
    let status = remove_current_executable(&executable)?;
    if cli.json {
        emit(json!({"path":path,"status":status,"purge":purge}), true);
    } else if status == "scheduled" {
        println!("Scheduled removal of {path} after this process exits");
    } else {
        println!("Removed {path}");
    }
    if let Some(purge) = purge {
        println!("Global config: {}", purge["configStatus"]);
        match purge["configDirectoryStatus"].as_str() {
            Some("not_empty") => println!(
                "Global config directory retained because it is not empty: {}",
                purge["configDirectory"]
            ),
            Some(status) => println!("Global config directory: {status}"),
            None => println!("Global config directory: unknown status"),
        }
        let removed_stores = purge["credentialStoresRemoved"]
            .as_array()
            .expect("credentialStoresRemoved is always an array");
        if removed_stores.is_empty() {
            println!("Stored credentials: none removed");
        } else {
            let stores = removed_stores
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ");
            println!("Stored credentials removed from: {stores}");
        }
    }
    Ok(())
}

fn run(cli: &Cli) -> Result<()> {
    if let Commands::Upgrade(args) = &cli.command {
        return upgrade(cli, args);
    }
    if let Commands::Uninstall(args) = &cli.command {
        return uninstall(cli, args);
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
        let cfg = config(&cli.cwd)?;
        return auth_login(cli, &cfg);
    }
    let cfg = config(&cli.cwd)?;
    let host = resolve_host(cli.host.as_deref(), env::var("OPENPROJECT_URL").ok(), &cfg)?;
    if let Commands::Auth {
        command: AuthCommands::Status,
    } = &cli.command
    {
        return auth_status(cli, &cfg, &host);
    }
    if let Commands::Auth {
        command: AuthCommands::Logout,
    } = &cli.command
    {
        return auth_logout(cli, &cfg, &host);
    }
    let token = resolve_token(&host, &cfg)?;
    let client = OpenProjectClient::new(host, token)?;
    match &cli.command {
        Commands::Auth {
            command: AuthCommands::Verify,
        } => emit(client.get("/users/me")?, cli.json),
        Commands::Projects(page) => emit(client.page("/projects", page)?, cli.json),
        Commands::Statuses(page) => emit(client.page("/statuses", page)?, cli.json),
        Commands::Priorities(page) => emit(client.page("/priorities", page)?, cli.json),
        Commands::Types(args)
        | Commands::Users(args)
        | Commands::Versions(args)
        | Commands::Categories(args) => {
            let project = resolve_project(&client, &cli.cwd, &cfg, args.project.as_deref())?;
            let project_id = id(&project)?;
            let suffix = match &cli.command {
                Commands::Types(_) => "types",
                Commands::Users(_) => "available_assignees",
                Commands::Versions(_) => "versions",
                Commands::Categories(_) => "categories",
                _ => unreachable!(),
            };
            emit(
                client.page(&format!("/projects/{project_id}/{suffix}"), &args.page)?,
                cli.json,
            );
        }
        Commands::Project(args) => {
            let project = resolve_project(&client, &cli.cwd, &cfg, args.project.as_deref())?;
            if args.bind {
                let path = project_config_path(&cli.cwd);
                if cli.dry_run {
                    emit(
                        json!({"dryRun":true, "project": project, "bind": path, "projectId": id(&project)?}),
                        cli.json,
                    );
                } else {
                    let path = bind_project(&cli.cwd, id(&project)?)?;
                    emit(json!({"project": project, "bound": path}), cli.json);
                }
            } else {
                emit(project, cli.json);
            }
        }
        Commands::Tasks(args) => {
            let project = resolve_project(&client, &cli.cwd, &cfg, args.project.as_deref())?;
            let project_id = id(&project)?;
            let assignee = args
                .assignee
                .as_deref()
                .map(|value| resolve_user(&client, value))
                .transpose()?;
            let statuses = resolve_items(&client, "/statuses", &args.status, "status")?;
            let types = resolve_items(
                &client,
                &format!("/projects/{project_id}/types"),
                &args.types,
                "type",
            )?;
            let priorities = resolve_items(&client, "/priorities", &args.priority, "priority")?;
            let path =
                work_package_path(project_id, args, assignee, &statuses, &types, &priorities)?;
            emit(
                page_elements(&client.page(&path, &args.page)?, &client.host),
                cli.json,
            );
        }
        Commands::Task(args) => {
            let task = client.get(&format!("/work_packages/{}", args.task_id))?;
            emit(
                if args.full {
                    task
                } else {
                    task_summary(&task, &client.host)
                },
                cli.json,
            );
        }
        Commands::Activities(args) => emit(activity_page(&client, args)?, cli.json),
        Commands::Activity(args) => emit(
            client.get(&format!("/activities/{}", args.activity_id))?,
            cli.json,
        ),
        Commands::TimeEntryActivities(args) => {
            emit(time_entry_activities(&client, args.task_id)?, cli.json)
        }
        Commands::Relations(args) => emit(relation_page(&client, args)?, cli.json),
        Commands::Relation { command } => match command {
            RelationCommands::Add(args) => emit(
                write(
                    &client,
                    cli,
                    reqwest::Method::POST,
                    &format!("/work_packages/{}/relations", args.from_id),
                    json!({
                        "type": args.r#type,
                        "description": args.description,
                        "lag": args.lag,
                        "_links": {
                            "to": {"href": format!("/api/v3/work_packages/{}", args.to)}
                        }
                    }),
                )?,
                cli.json,
            ),
            RelationCommands::Delete(args) => emit(
                write_without_body(
                    &client,
                    cli,
                    reqwest::Method::DELETE,
                    &format!("/relations/{}", args.relation_id),
                )?,
                cli.json,
            ),
        },
        Commands::Create(args) => {
            let project = resolve_project(&client, &cli.cwd, &cfg, args.project.as_deref())?;
            let project_id = id(&project)?;
            let type_id = match args.type_id {
                Some(value) => value,
                None => resolve_item(
                    &client,
                    &format!("/projects/{project_id}/types"),
                    &args.r#type,
                    "type",
                )?,
            };
            let assignee = args
                .assignee
                .as_deref()
                .map(|a| resolve_user(&client, a))
                .transpose()?;
            let priority = args
                .priority
                .as_deref()
                .map(|value| resolve_item(&client, "/priorities", value, "priority"))
                .transpose()?;
            let responsible = args
                .responsible
                .as_deref()
                .map(|value| resolve_user(&client, value))
                .transpose()?;
            let version = args
                .version
                .as_deref()
                .map(|value| {
                    resolve_item(
                        &client,
                        &format!("/projects/{project_id}/versions"),
                        value,
                        "version",
                    )
                })
                .transpose()?;
            let (custom_fields, custom_links) =
                custom_field_operations(&args.custom_fields, &args.custom_field_links, &[], &[])?;
            let mut links = Map::new();
            links.insert(
                "project".into(),
                json!({"href":format!("/api/v3/projects/{project_id}")}),
            );
            links.insert(
                "type".into(),
                json!({"href":format!("/api/v3/types/{type_id}")}),
            );
            if let Some(value) = assignee {
                links.insert(
                    "assignee".into(),
                    json!({"href":format!("/api/v3/users/{value}")}),
                );
            }
            if let Some(value) = priority {
                links.insert(
                    "priority".into(),
                    json!({"href":format!("/api/v3/priorities/{value}")}),
                );
            }
            if let Some(value) = responsible {
                links.insert(
                    "responsible".into(),
                    json!({"href":format!("/api/v3/users/{value}")}),
                );
            }
            if let Some(value) = args.parent {
                links.insert(
                    "parent".into(),
                    json!({"href":format!("/api/v3/work_packages/{value}")}),
                );
            }
            if let Some(value) = version {
                links.insert(
                    "version".into(),
                    json!({"href":format!("/api/v3/versions/{value}")}),
                );
            }
            links.extend(custom_links);
            let mut payload = custom_fields;
            payload.insert("subject".into(), json!(args.subject));
            payload.insert(
                "description".into(),
                args.description
                    .as_ref()
                    .map(|raw| json!({"format":"markdown","raw":raw}))
                    .unwrap_or(Value::Null),
            );
            payload.insert("startDate".into(), json!(args.start_date));
            payload.insert("dueDate".into(), json!(args.due_date));
            payload.insert(
                "estimatedTime".into(),
                args.estimate
                    .as_deref()
                    .map(duration)
                    .transpose()?
                    .map(Value::String)
                    .unwrap_or(Value::Null),
            );
            payload.insert("_links".into(), Value::Object(links));
            emit(
                write(
                    &client,
                    cli,
                    reqwest::Method::POST,
                    "/work_packages",
                    Value::Object(payload),
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
            let priority = args
                .priority
                .as_deref()
                .map(|value| resolve_item(&client, "/priorities", value, "priority"))
                .transpose()?;
            let responsible = args
                .responsible
                .as_deref()
                .map(|value| resolve_user(&client, value))
                .transpose()?;
            let version_path = href(&current, "project")
                .map(|path| format!("{path}/versions"))
                .unwrap_or_else(|| "/versions".to_owned());
            let version = args
                .version
                .as_deref()
                .map(|value| resolve_item(&client, &version_path, value, "version"))
                .transpose()?;
            let (custom_fields, custom_links) = custom_field_operations(
                &args.custom_fields,
                &args.custom_field_links,
                &args.clear_custom_fields,
                &args.clear_custom_field_links,
            )?;
            if args.subject.is_none()
                && args.description.is_none()
                && status.is_none()
                && assignee.is_none()
                && priority.is_none()
                && responsible.is_none()
                && args.parent.is_none()
                && version.is_none()
                && args.percent.is_none()
                && args.start_date.is_none()
                && args.due_date.is_none()
                && args.estimate.is_none()
                && custom_fields.is_empty()
                && custom_links.is_empty()
                && !args.clear_description
                && !args.clear_assignee
                && !args.clear_responsible
                && !args.clear_parent
                && !args.clear_version
                && !args.clear_start_date
                && !args.clear_due_date
                && !args.clear_estimate
            {
                bail!("no update fields were supplied");
            }
            if args.assignee.is_some() && args.clear_assignee
                || args.description.is_some() && args.clear_description
                || args.responsible.is_some() && args.clear_responsible
                || args.parent.is_some() && args.clear_parent
                || args.version.is_some() && args.clear_version
                || args.start_date.is_some() && args.clear_start_date
                || args.due_date.is_some() && args.clear_due_date
                || args.estimate.is_some() && args.clear_estimate
            {
                bail!("a value and its corresponding --clear-* option cannot be used together");
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
            if args.clear_assignee {
                links.insert("assignee".into(), Value::Null);
            }
            if let Some(n) = priority {
                links.insert(
                    "priority".into(),
                    json!({"href":format!("/api/v3/priorities/{n}")}),
                );
            }
            if let Some(n) = responsible {
                links.insert(
                    "responsible".into(),
                    json!({"href":format!("/api/v3/users/{n}")}),
                );
            }
            if args.clear_responsible {
                links.insert("responsible".into(), Value::Null);
            }
            if let Some(n) = args.parent {
                links.insert(
                    "parent".into(),
                    json!({"href":format!("/api/v3/work_packages/{n}")}),
                );
            }
            if args.clear_parent {
                links.insert("parent".into(), Value::Null);
            }
            if let Some(n) = version {
                links.insert(
                    "version".into(),
                    json!({"href":format!("/api/v3/versions/{n}")}),
                );
            }
            if args.clear_version {
                links.insert("version".into(), Value::Null);
            }
            links.extend(custom_links);
            let mut payload = custom_fields;
            payload.insert(
                "lockVersion".into(),
                current.get("lockVersion").cloned().unwrap_or(Value::Null),
            );
            payload.insert("_links".into(), Value::Object(links));
            if let Some(subject) = &args.subject {
                payload.insert("subject".into(), Value::String(subject.clone()));
            }
            if let Some(description) = &args.description {
                payload.insert(
                    "description".into(),
                    json!({"format":"markdown","raw":description}),
                );
            }
            if args.clear_description {
                payload.insert("description".into(), Value::Null);
            }
            if let Some(percent) = args.percent {
                payload.insert("percentageDone".into(), json!(percent));
            }
            if let Some(start_date) = &args.start_date {
                payload.insert("startDate".into(), json!(start_date));
            }
            if args.clear_start_date {
                payload.insert("startDate".into(), Value::Null);
            }
            if let Some(due_date) = &args.due_date {
                payload.insert("dueDate".into(), json!(due_date));
            }
            if args.clear_due_date {
                payload.insert("dueDate".into(), Value::Null);
            }
            if let Some(estimate) = &args.estimate {
                payload.insert("estimatedTime".into(), json!(duration(estimate)?));
            }
            if args.clear_estimate {
                payload.insert("estimatedTime".into(), Value::Null);
            }
            emit(
                write_exact(
                    &client,
                    cli,
                    reqwest::Method::PATCH,
                    &format!("/work_packages/{}", args.task_id),
                    Value::Object(payload),
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
            let activity = args
                .activity
                .as_deref()
                .map(|value| resolve_time_entry_activity(&client, args.task_id, value))
                .transpose()?;
            let payload = json!({"hours":duration(&args.hours)?,"spentOn":args.date,"comment":args.comment.as_ref().map(|raw|json!({"format":"plain","raw":raw})),"_links":{"workPackage":{"href":format!("/api/v3/work_packages/{}",args.task_id)},"project":{"href":project},"activity":activity.map(|href|json!({"href":href}))}});
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
        | Commands::Auth {
            command: AuthCommands::Status | AuthCommands::Logout,
        }
        | Commands::CommitLink(_)
        | Commands::Upgrade(_)
        | Commands::Uninstall(_) => unreachable!(),
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
    use std::collections::HashMap;

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

        fn delete(&self) -> Result<bool> {
            Ok(self.token.borrow_mut().take().is_some())
        }
    }

    #[test]
    fn reads_time_entry_activities_from_form_schema() {
        let form = json!({
            "_embedded": {
                "schema": {
                    "activity": {
                        "_embedded": {
                            "allowedValues": [{
                                "id": 18,
                                "name": "Development",
                                "_links": {"self": {"href": "/api/v3/time_entries/activities/18"}}
                            }]
                        }
                    }
                }
            }
        });

        assert_eq!(
            time_entry_activity_values(&form).unwrap(),
            vec![json!({
                "id": 18,
                "name": "Development",
                "href": "/api/v3/time_entries/activities/18"
            })]
        );
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
    fn task_list_uses_server_side_filters_and_pagination() {
        let cli = Cli::try_parse_from([
            "openproject",
            "tasks",
            "--assignee",
            "7",
            "--query",
            "approval",
            "--limit",
            "25",
            "--offset",
            "3",
        ])
        .unwrap();
        let Commands::Tasks(args) = cli.command else {
            panic!("expected tasks command");
        };
        let path = work_package_path(13, &args, Some(7), &[], &[], &[]).unwrap();
        let filters = url::form_urlencoded::parse(path.split_once('?').unwrap().1.as_bytes())
            .find_map(|(key, value)| (key == "filters").then_some(value.into_owned()))
            .unwrap();
        assert!(filters.contains("\"status\":{\"operator\":\"o\""));
        assert!(filters.contains("\"assignee\":{\"operator\":\"=\""));
        assert!(filters.contains("approval"));
        assert_eq!(args.page.limit, 25);
        assert_eq!(args.page.offset, 3);
    }

    #[test]
    fn task_list_supports_advanced_filters_and_sorting() {
        let cli = Cli::try_parse_from([
            "openproject",
            "tasks",
            "--status",
            "In progress",
            "--type",
            "Bug",
            "--priority",
            "High",
            "--due-before",
            "2026-09-30",
            "--updated-since",
            "7d",
            "--sort",
            "priority:desc",
            "--sort",
            "updated-at:desc",
        ])
        .unwrap();
        let Commands::Tasks(args) = cli.command else {
            panic!("expected tasks command");
        };

        let path = work_package_path(13, &args, None, &[5], &[2], &[9]).unwrap();
        let parameters = url::form_urlencoded::parse(path.split_once('?').unwrap().1.as_bytes())
            .into_owned()
            .collect::<HashMap<String, String>>();
        let filters: Value = serde_json::from_str(&parameters["filters"]).unwrap();
        let sort_by: Value = serde_json::from_str(&parameters["sortBy"]).unwrap();

        assert_eq!(filters[0], json!({"status":{"operator":"=","values":[5]}}));
        assert!(filters
            .as_array()
            .unwrap()
            .contains(&json!({"dueDate":{"operator":"<=d","values":["2026-09-30"]}})));
        assert!(filters
            .as_array()
            .unwrap()
            .contains(&json!({"updatedAt":{"operator":">t-","values":[7]}})));
        assert_eq!(
            sort_by,
            json!([["priority", "desc"], ["updatedAt", "desc"]])
        );
    }

    #[test]
    fn custom_field_operations_keep_scalar_and_link_values_separate() {
        let (properties, links) = custom_field_operations(
            &["customField1=true".into(), "customField2=Acme".into()],
            &["customField3=/api/v3/users/14".into()],
            &["customField4".into()],
            &["customField5".into()],
        )
        .unwrap();

        assert_eq!(properties["customField1"], json!(true));
        assert_eq!(properties["customField2"], json!("Acme"));
        assert!(properties["customField4"].is_null());
        assert_eq!(links["customField3"], json!({"href":"/api/v3/users/14"}));
        assert!(links["customField5"].is_null());
    }

    #[test]
    fn duplicate_custom_field_operations_are_rejected() {
        let error = custom_field_operations(
            &["customField1=value".into()],
            &[],
            &["customField1".into()],
            &[],
        )
        .unwrap_err();

        assert!(error.to_string().contains("supplied more than once"));
    }

    #[test]
    fn update_clear_options_are_explicit() {
        let cli = Cli::try_parse_from([
            "openproject",
            "update",
            "12",
            "--clear-description",
            "--clear-assignee",
        ])
        .unwrap();
        let Commands::Update(args) = cli.command else {
            panic!("expected update command");
        };
        assert!(args.clear_description);
        assert!(args.clear_assignee);
        assert!(args.description.is_none());
        assert!(args.assignee.is_none());
    }

    #[test]
    fn project_candidates_include_readme_and_manifest_names() {
        let directory = env::temp_dir().join(format!(
            "openproject-project-candidates-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("README.md"), "# Readme Project\n").unwrap();
        fs::write(
            directory.join("Cargo.toml"),
            "[package]\nname = \"cargo-project\"\n",
        )
        .unwrap();
        fs::write(directory.join("package.json"), r#"{"name":"node-project"}"#).unwrap();

        let candidates = project_candidates(&directory);

        assert!(candidates.iter().any(|value| value == "Readme Project"));
        assert!(candidates.iter().any(|value| value == "cargo-project"));
        assert!(candidates.iter().any(|value| value == "node-project"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn project_relevance_suggests_related_names_but_not_generic_words() {
        assert!(project_relevance("customer-portal-app", "Customer Portal").is_some());
        assert!(project_relevance("billing", "Billing Service").is_some());
        assert!(project_relevance("api-client", "API Service").is_none());
    }

    #[test]
    fn uninstall_dry_run_does_not_require_openproject_credentials() {
        let cli = Cli::try_parse_from(["openproject", "uninstall", "--dry-run", "--json"]).unwrap();
        assert!(run(&cli).is_ok());
    }

    #[test]
    fn uninstall_purge_is_opt_in() {
        let cli = Cli::try_parse_from(["openproject", "uninstall", "--purge"]).unwrap();
        let Commands::Uninstall(args) = cli.command else {
            panic!("expected uninstall command");
        };
        assert!(args.purge);
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
                project: Some("13".into()),
                credential_store: None,
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
            credential_store: None,
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
            credential_store: None,
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
    fn credential_store_can_delete_a_token() {
        let store = MemoryCredentialStore::empty();
        store.save("stored-token").unwrap();
        assert!(store.delete().unwrap());
        assert_eq!(store.load().unwrap(), None);
        assert!(!store.delete().unwrap());
    }

    #[test]
    fn file_credentials_are_host_scoped_and_replaceable() {
        let directory =
            env::temp_dir().join(format!("openproject-credentials-{}", std::process::id()));
        let path = directory.join("credentials.json");
        let one = FileCredentialStore::at("https://one.example.com", path.clone());
        let two = FileCredentialStore::at("https://two.example.com", path.clone());
        one.save("one").unwrap();
        #[cfg(unix)]
        {
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        two.save("two").unwrap();
        one.save("new-one").unwrap();
        assert_eq!(one.load().unwrap().as_deref(), Some("new-one"));
        assert_eq!(two.load().unwrap().as_deref(), Some("two"));
        assert!(one.delete().unwrap());
        assert!(one.load().unwrap().is_none());
        assert!(two.delete().unwrap());
        assert!(!path.exists());
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn config_accepts_the_selected_store_but_project_config_cannot_set_it() {
        let global = object(json!({"host":"https://example.com","credential_store":"file"}));
        let config = config_from_maps(
            &global,
            Path::new("global.json"),
            &Map::new(),
            Path::new(".openproject.json"),
        )
        .unwrap();
        assert_eq!(config.credential_store, Some(CredentialStoreKind::File));
        let project = object(json!({"credential_store":"file"}));
        assert!(config_from_maps(
            &Map::new(),
            Path::new("global.json"),
            &project,
            Path::new(".openproject.json")
        )
        .is_err());
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
