use crate::client::OpenProjectClient;
use crate::PageArgs;
use anyhow::{anyhow, bail, Context, Result};
use clap::{Args, Subcommand};
use reqwest::blocking::multipart::{Form, Part};
use serde_json::{json, Value};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

#[derive(Args, Debug)]
pub(crate) struct AttachmentsArgs {
    pub(crate) task_id: u64,
    #[command(flatten)]
    pub(crate) page: PageArgs,
}

#[derive(Subcommand, Debug)]
pub(crate) enum AttachmentCommands {
    /// Show attachment metadata.
    Show(AttachmentIdArgs),
    /// Upload a file to a work package.
    Upload(AttachmentUploadArgs),
    /// Download an attachment to a local file.
    Download(AttachmentDownloadArgs),
    /// Permanently delete an attachment.
    Delete(AttachmentIdArgs),
}

#[derive(Args, Debug)]
pub(crate) struct AttachmentIdArgs {
    pub(crate) attachment_id: u64,
}

#[derive(Args, Debug)]
pub(crate) struct AttachmentUploadArgs {
    pub(crate) task_id: u64,
    pub(crate) file: PathBuf,
    /// Override the filename stored in OpenProject.
    #[arg(long)]
    pub(crate) name: Option<String>,
    /// Plain-text attachment description.
    #[arg(long)]
    pub(crate) description: Option<String>,
    /// Override the detected MIME content type.
    #[arg(long)]
    pub(crate) content_type: Option<String>,
}

#[derive(Args, Debug)]
pub(crate) struct AttachmentDownloadArgs {
    pub(crate) attachment_id: u64,
    /// Exact destination file path. Defaults to the server filename in the current directory.
    #[arg(long)]
    pub(crate) output: Option<PathBuf>,
    /// Replace an existing regular file. Symbolic links are never followed.
    #[arg(long)]
    pub(crate) force: bool,
}

pub(crate) fn list(client: &OpenProjectClient, args: &AttachmentsArgs) -> Result<Value> {
    client.page(
        &format!("/work_packages/{}/attachments", args.task_id),
        &args.page,
    )
}

pub(crate) fn show(client: &OpenProjectClient, args: &AttachmentIdArgs) -> Result<Value> {
    client.get(&format!("/attachments/{}", args.attachment_id))
}

pub(crate) fn upload(
    client: &OpenProjectClient,
    args: &AttachmentUploadArgs,
    dry_run: bool,
) -> Result<Value> {
    let local = upload_file(
        &args.file,
        args.name.as_deref(),
        args.content_type.as_deref(),
    )?;
    let path = format!("/work_packages/{}/attachments", args.task_id);
    if dry_run {
        return Ok(json!({
            "dryRun": true,
            "method": "POST",
            "path": path,
            "file": {
                "path": local.path,
                "fileName": local.name,
                "fileSize": local.size,
                "contentType": local.content_type,
            },
            "description": args.description,
        }));
    }

    let mut metadata = json!({"fileName": local.name});
    if let Some(description) = &args.description {
        metadata["description"] = json!({"format":"plain","raw":description});
    }
    let metadata_part = Part::text(metadata.to_string())
        .mime_str("application/json")
        .context("cannot build attachment metadata")?;
    let file_part = Part::file(&local.path)
        .context("cannot open attachment file")?
        .file_name(local.name)
        .mime_str(&local.content_type)
        .context("invalid attachment content type")?;
    let form = Form::new()
        .part("metadata", metadata_part)
        .part("file", file_part);
    client.post_multipart(&path, form)
}

pub(crate) fn download(
    client: &OpenProjectClient,
    args: &AttachmentDownloadArgs,
    dry_run: bool,
) -> Result<Value> {
    let attachment = client.get(&format!("/attachments/{}", args.attachment_id))?;
    let server_name = attachment
        .get("fileName")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("attachment response has no fileName"))?;
    validate_file_name(server_name)?;
    let destination = destination_path(args.output.as_deref(), server_name)?;
    validate_destination(&destination, args.force)?;
    let href = attachment
        .pointer("/_links/staticDownloadLocation/href")
        .or_else(|| attachment.pointer("/_links/downloadLocation/href"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("attachment response has no download location"))?;

    if dry_run {
        return Ok(json!({
            "dryRun": true,
            "attachment": {"id": args.attachment_id, "fileName": server_name},
            "path": destination,
            "force": args.force,
        }));
    }

    let parent = destination
        .parent()
        .ok_or_else(|| anyhow!("attachment destination has no parent directory"))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .context("cannot create temporary attachment file")?;
    let bytes = client.download(href, temporary.as_file_mut())?;
    temporary
        .as_file_mut()
        .flush()
        .context("cannot flush attachment download")?;
    if let Some(expected) = attachment.get("fileSize").and_then(Value::as_u64) {
        if bytes != expected {
            bail!("attachment download size mismatch: expected {expected} bytes, received {bytes}");
        }
    }

    if args.force {
        temporary
            .persist(&destination)
            .map_err(|error| error.error)
            .context("cannot replace attachment destination")?;
    } else {
        temporary
            .persist_noclobber(&destination)
            .map_err(|error| error.error)
            .context("cannot save attachment without overwriting")?;
    }

    Ok(json!({
        "attachment": {"id": args.attachment_id, "fileName": server_name},
        "path": destination,
        "bytes": bytes,
        "contentType": attachment.get("contentType"),
    }))
}

pub(crate) fn delete(
    client: &OpenProjectClient,
    args: &AttachmentIdArgs,
    dry_run: bool,
) -> Result<Value> {
    let path = format!("/attachments/{}", args.attachment_id);
    if dry_run {
        return Ok(json!({"dryRun":true,"method":"DELETE","path":path}));
    }
    client.request(reqwest::Method::DELETE, &path, None)?;
    Ok(json!({"deleted":true,"attachmentId":args.attachment_id}))
}

struct UploadFile {
    path: PathBuf,
    name: String,
    size: u64,
    content_type: String,
}

fn upload_file(path: &Path, name: Option<&str>, content_type: Option<&str>) -> Result<UploadFile> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("cannot inspect attachment file {}", path.display()))?;
    if !metadata.is_file() {
        bail!("attachment path must be a regular file: {}", path.display());
    }
    let name = match name {
        Some(name) => name.to_owned(),
        None => path
            .file_name()
            .and_then(|value| value.to_str())
            .map(str::to_owned)
            .ok_or_else(|| anyhow!("attachment file has no valid UTF-8 filename"))?,
    };
    validate_file_name(&name)?;
    let content_type = content_type.map(str::to_owned).unwrap_or_else(|| {
        mime_guess::from_path(path)
            .first_or_octet_stream()
            .essence_str()
            .to_owned()
    });
    content_type
        .parse::<mime_guess::mime::Mime>()
        .context("invalid attachment content type")?;

    Ok(UploadFile {
        path: path.to_path_buf(),
        name,
        size: metadata.len(),
        content_type,
    })
}

fn validate_file_name(name: &str) -> Result<()> {
    if name.is_empty() || name.contains('/') || name.contains('\\') {
        bail!("attachment filename must be a single non-empty path component");
    }
    let mut components = Path::new(name).components();
    if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
        bail!("attachment filename must be a single non-empty path component");
    }
    Ok(())
}

fn destination_path(output: Option<&Path>, server_name: &str) -> Result<PathBuf> {
    let current = std::env::current_dir().context("cannot determine current directory")?;
    let path = output
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(server_name));
    let path = if path.is_absolute() {
        path
    } else {
        current.join(path)
    };
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("attachment destination has no parent directory"))?;
    if !parent.is_dir() {
        bail!(
            "attachment destination directory does not exist: {}",
            parent.display()
        );
    }
    Ok(path)
}

fn validate_destination(path: &Path, force: bool) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!("refusing to overwrite symbolic link: {}", path.display())
        }
        Ok(metadata) if !metadata.is_file() => {
            bail!(
                "attachment destination is not a regular file: {}",
                path.display()
            )
        }
        Ok(_) if !force => bail!(
            "attachment destination already exists; pass --force to replace it: {}",
            path.display()
        ),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error)
            .with_context(|| format!("cannot inspect attachment destination {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc;

    fn read_request(stream: &mut TcpStream) -> Vec<u8> {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];
        let header_end = loop {
            let read = stream.read(&mut buffer).unwrap();
            if read == 0 {
                return request;
            }
            request.extend_from_slice(&buffer[..read]);
            if let Some(position) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                break position + 4;
            }
        };
        let headers = String::from_utf8_lossy(&request[..header_end]);
        let content_length = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .and_then(|value| value.trim().parse::<usize>().ok())
            })
            .unwrap_or(0);
        while request.len() < header_end + content_length {
            let read = stream.read(&mut buffer).unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
        }
        request
    }

    fn respond_json(stream: &mut TcpStream, body: &str) {
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/hal+json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        )
        .unwrap();
    }

    fn respond_empty(stream: &mut TcpStream) {
        stream
            .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
            .unwrap();
    }

    #[test]
    fn attachment_file_names_are_single_safe_components() {
        for name in ["evidence.txt", "release notes.pdf"] {
            validate_file_name(name).unwrap();
        }
        for name in ["", ".", "..", "../secret", "folder/file", "folder\\file"] {
            assert!(
                validate_file_name(name).is_err(),
                "{name} should be rejected"
            );
        }
    }

    #[test]
    fn upload_file_detects_mime_and_allows_valid_override() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("evidence.txt");
        fs::write(&path, b"proof").unwrap();
        let detected = upload_file(&path, None, None).unwrap();
        assert_eq!(detected.name, "evidence.txt");
        assert_eq!(detected.size, 5);
        assert_eq!(detected.content_type, "text/plain");
        let overridden = upload_file(&path, Some("proof.log"), Some("text/x-log")).unwrap();
        assert_eq!(overridden.name, "proof.log");
        assert_eq!(overridden.content_type, "text/x-log");
        assert!(upload_file(&path, None, Some("not a mime")).is_err());
    }

    #[test]
    fn destination_requires_force_and_never_accepts_symlinks() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("existing.txt");
        fs::write(&path, b"old").unwrap();
        assert!(validate_destination(&path, false).is_err());
        validate_destination(&path, true).unwrap();

        #[cfg(unix)]
        {
            let link = directory.path().join("link.txt");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            assert!(validate_destination(&link, true).is_err());
        }
    }

    #[test]
    fn upload_sends_expected_multipart_parts() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            tx.send(read_request(&mut stream)).unwrap();
            respond_json(&mut stream, r#"{"id":9,"fileName":"proof.txt"}"#);
        });
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("proof.txt");
        fs::write(&path, b"evidence-body").unwrap();
        let client = OpenProjectClient::new(format!("http://{address}"), "token".into()).unwrap();
        let result = upload(
            &client,
            &AttachmentUploadArgs {
                task_id: 42,
                file: path,
                name: None,
                description: Some("Build evidence".into()),
                content_type: None,
            },
            false,
        )
        .unwrap();
        assert_eq!(result["id"], 9);

        let request = String::from_utf8(rx.recv().unwrap()).unwrap();
        assert!(request.starts_with("POST /api/v3/work_packages/42/attachments "));
        assert!(request
            .to_ascii_lowercase()
            .contains("authorization: bearer token"));
        assert!(request.contains("name=\"metadata\""));
        assert!(request.contains("name=\"file\""));
        assert!(request.contains(r#""fileName":"proof.txt""#));
        assert!(request.contains("evidence-body"));
        server.join().unwrap();
    }

    #[test]
    fn list_show_and_delete_use_the_documented_endpoints() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        let server = std::thread::spawn(move || {
            for request_number in 0..3 {
                let (mut stream, _) = listener.accept().unwrap();
                tx.send(read_request(&mut stream)).unwrap();
                match request_number {
                    0 => respond_json(&mut stream, r#"{"_embedded":{"elements":[]}}"#),
                    1 => respond_json(&mut stream, r#"{"id":9,"fileName":"proof.txt"}"#),
                    _ => respond_empty(&mut stream),
                }
            }
        });
        let client = OpenProjectClient::new(format!("http://{address}"), "token".into()).unwrap();
        list(
            &client,
            &AttachmentsArgs {
                task_id: 42,
                page: PageArgs {
                    limit: 25,
                    offset: 2,
                },
            },
        )
        .unwrap();
        show(&client, &AttachmentIdArgs { attachment_id: 9 }).unwrap();
        let deleted = delete(&client, &AttachmentIdArgs { attachment_id: 9 }, false).unwrap();
        assert_eq!(deleted, json!({"deleted":true,"attachmentId":9}));

        let requests = (0..3)
            .map(|_| String::from_utf8(rx.recv().unwrap()).unwrap())
            .collect::<Vec<_>>();
        assert!(requests[0]
            .starts_with("GET /api/v3/work_packages/42/attachments?pageSize=25&offset=2 "));
        assert!(requests[1].starts_with("GET /api/v3/attachments/9 "));
        assert!(requests[2].starts_with("DELETE /api/v3/attachments/9 "));
        server.join().unwrap();
    }

    #[test]
    fn upload_and_delete_dry_runs_do_not_contact_the_server() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("proof.txt");
        fs::write(&path, b"proof").unwrap();
        let client = OpenProjectClient::new("http://127.0.0.1:1".into(), "token".into()).unwrap();
        let upload_result = upload(
            &client,
            &AttachmentUploadArgs {
                task_id: 42,
                file: path,
                name: None,
                description: None,
                content_type: None,
            },
            true,
        )
        .unwrap();
        assert_eq!(upload_result["dryRun"], true);
        let delete_result = delete(&client, &AttachmentIdArgs { attachment_id: 9 }, true).unwrap();
        assert_eq!(delete_result["dryRun"], true);
    }

    #[test]
    fn download_streams_to_an_atomic_destination() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for request_number in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let _request = read_request(&mut stream);
                if request_number == 0 {
                    respond_json(
                        &mut stream,
                        r#"{"id":7,"fileName":"proof.txt","fileSize":5,"contentType":"text/plain","_links":{"staticDownloadLocation":{"href":"/api/v3/attachments/7/content"}}}"#,
                    );
                } else {
                    stream
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nproof",
                        )
                        .unwrap();
                }
            }
        });
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("downloaded.txt");
        let client = OpenProjectClient::new(format!("http://{address}"), "token".into()).unwrap();
        let result = download(
            &client,
            &AttachmentDownloadArgs {
                attachment_id: 7,
                output: Some(destination.clone()),
                force: false,
            },
            false,
        )
        .unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"proof");
        assert_eq!(result["bytes"], 5);
        server.join().unwrap();
    }

    #[test]
    fn download_dry_run_fetches_metadata_but_creates_no_file() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _request = read_request(&mut stream);
            respond_json(
                &mut stream,
                r#"{"id":7,"fileName":"proof.txt","fileSize":5,"_links":{"staticDownloadLocation":{"href":"/api/v3/attachments/7/content"}}}"#,
            );
        });
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("dry-run.txt");
        let client = OpenProjectClient::new(format!("http://{address}"), "token".into()).unwrap();
        let result = download(
            &client,
            &AttachmentDownloadArgs {
                attachment_id: 7,
                output: Some(destination.clone()),
                force: false,
            },
            true,
        )
        .unwrap();
        assert_eq!(result["dryRun"], true);
        assert!(!destination.exists());
        server.join().unwrap();
    }

    #[test]
    fn size_mismatch_leaves_no_destination_file() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            for request_number in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let _request = read_request(&mut stream);
                if request_number == 0 {
                    respond_json(
                        &mut stream,
                        r#"{"id":8,"fileName":"proof.txt","fileSize":6,"_links":{"staticDownloadLocation":{"href":"/api/v3/attachments/8/content"}}}"#,
                    );
                } else {
                    stream
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nproof",
                        )
                        .unwrap();
                }
            }
        });
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("mismatch.txt");
        let client = OpenProjectClient::new(format!("http://{address}"), "token".into()).unwrap();
        let error = download(
            &client,
            &AttachmentDownloadArgs {
                attachment_id: 8,
                output: Some(destination.clone()),
                force: false,
            },
            false,
        )
        .unwrap_err();
        assert!(error.to_string().contains("size mismatch"));
        assert!(!destination.exists());
        server.join().unwrap();
    }
}
