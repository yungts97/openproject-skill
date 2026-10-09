use crate::{canonical_host, PageArgs};
use anyhow::{bail, Context, Result};
use reqwest::blocking::{multipart::Form, Client as HttpClient, RequestBuilder, Response};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, LOCATION};
use serde_json::{json, Value};
use std::io::Write;
use std::time::Duration;
use url::Url;

const API_ACCEPT: &str = "application/hal+json, application/json";
const MAX_REDIRECTS: usize = 10;

pub(crate) struct OpenProjectClient {
    pub(crate) host: String,
    base: String,
    http: HttpClient,
    download_http: HttpClient,
    token: String,
}

impl OpenProjectClient {
    pub(crate) fn new(host: String, token: String) -> Result<Self> {
        let host = canonical_host(&host)?;
        Ok(Self {
            base: format!("{host}/api/v3"),
            host,
            http: HttpClient::builder()
                .timeout(Duration::from_secs(30))
                .build()?,
            download_http: HttpClient::builder()
                .timeout(Duration::from_secs(300))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            token,
        })
    }

    fn url(&self, path: &str) -> Result<String> {
        if path.starts_with("http://") || path.starts_with("https://") {
            let url = Url::parse(path)?;
            let expected = Url::parse(&self.host)?;
            if !same_origin(&url, &expected) {
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

    pub(crate) fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value> {
        let url = self.url(path)?;
        let retries = if method == reqwest::Method::GET { 2 } else { 0 };
        let mut attempt = 0;
        let response = loop {
            let mut request: RequestBuilder = self
                .http
                .request(method.clone(), &url)
                .header(ACCEPT, API_ACCEPT)
                .header(AUTHORIZATION, format!("Bearer {}", self.token));
            // OpenProject requires a Content-Type header even for bodyless DELETEs.
            if method == reqwest::Method::DELETE {
                request = request.header(CONTENT_TYPE, "application/json");
            }
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
                    std::thread::sleep(Duration::from_secs(wait));
                    attempt += 1;
                }
                Ok(response) => break response,
                Err(_error) if attempt < retries => {
                    attempt += 1;
                    std::thread::sleep(Duration::from_secs(attempt));
                }
                Err(error) => return Err(error).context("cannot connect to OpenProject"),
            }
        };
        json_response(response)
    }

    pub(crate) fn get(&self, path: &str) -> Result<Value> {
        self.request(reqwest::Method::GET, path, None)
    }

    pub(crate) fn collection(&self, path: &str) -> Result<Vec<Value>> {
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

    pub(crate) fn page(&self, path: &str, page: &PageArgs) -> Result<Value> {
        let query = format!("pageSize={}&offset={}", page.limit, page.offset);
        self.get(&format!(
            "{path}{}{}",
            if path.contains('?') { "&" } else { "?" },
            query
        ))
    }

    pub(crate) fn post_multipart(&self, path: &str, form: Form) -> Result<Value> {
        let response = self
            .http
            .post(self.url(path)?)
            .header(ACCEPT, API_ACCEPT)
            .header(AUTHORIZATION, format!("Bearer {}", self.token))
            .multipart(form)
            .send()
            .context("cannot upload attachment to OpenProject")?;
        json_response(response)
    }

    pub(crate) fn download(&self, href: &str, output: &mut impl Write) -> Result<u64> {
        let host = Url::parse(&self.host)?;
        let mut url = resolve_download_url(&host, href)?;

        for _ in 0..=MAX_REDIRECTS {
            let mut request = self.download_http.get(url.clone()).header(ACCEPT, "*/*");
            if same_origin(&url, &host) {
                request = request.header(AUTHORIZATION, format!("Bearer {}", self.token));
            }
            let mut response = request
                .send()
                .with_context(|| format!("cannot download attachment from {url}"))?;

            if response.status().is_redirection() {
                let location = response
                    .headers()
                    .get(LOCATION)
                    .context("attachment download redirect has no Location header")?
                    .to_str()
                    .context("attachment download redirect has an invalid Location header")?;
                url = url
                    .join(location)
                    .context("attachment download redirect URL is invalid")?;
                if !matches!(url.scheme(), "http" | "https") {
                    bail!("attachment download URL must use http or https");
                }
                continue;
            }
            if !response.status().is_success() {
                return json_response(response).map(|_| 0);
            }
            return std::io::copy(&mut response, output)
                .context("cannot write attachment download");
        }
        bail!("attachment download exceeded {MAX_REDIRECTS} redirects")
    }
}

fn resolve_download_url(host: &Url, href: &str) -> Result<Url> {
    let url = match Url::parse(href) {
        Ok(url) => url,
        Err(url::ParseError::RelativeUrlWithoutBase)
            if href == "/api/v3" || href.starts_with("/api/v3/") =>
        {
            Url::parse(&format!("{}{href}", host.as_str().trim_end_matches('/')))?
        }
        Err(url::ParseError::RelativeUrlWithoutBase) => host.join(href)?,
        Err(error) => return Err(error.into()),
    };
    if !matches!(url.scheme(), "http" | "https") {
        bail!("attachment download URL must use http or https");
    }
    Ok(url)
}

fn same_origin(left: &Url, right: &Url) -> bool {
    left.scheme() == right.scheme()
        && left.host_str() == right.host_str()
        && left.port_or_known_default() == right.port_or_known_default()
}

fn json_response(response: Response) -> Result<Value> {
    let status = response.status();
    let text = response.text().unwrap_or_default();
    if !status.is_success() {
        let detail = serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|value| {
                value
                    .get("message")
                    .or_else(|| value.get("errorIdentifier"))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    fn read_headers(stream: &mut std::net::TcpStream) -> String {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = stream.read(&mut buffer).unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
        }
        String::from_utf8(request).unwrap()
    }

    #[test]
    fn origin_comparison_includes_scheme_host_and_port() {
        let origin = Url::parse("https://openproject.example.com").unwrap();
        assert!(same_origin(
            &Url::parse("https://openproject.example.com/api/v3").unwrap(),
            &origin
        ));
        assert!(!same_origin(
            &Url::parse("http://openproject.example.com/api/v3").unwrap(),
            &origin
        ));
        assert!(!same_origin(
            &Url::parse("https://files.example.com/file").unwrap(),
            &origin
        ));
    }

    #[test]
    fn download_urls_allow_only_http_and_https() {
        let origin = Url::parse("https://openproject.example.com").unwrap();
        assert_eq!(
            resolve_download_url(&origin, "/api/v3/attachments/1/content")
                .unwrap()
                .as_str(),
            "https://openproject.example.com/api/v3/attachments/1/content"
        );
        assert!(resolve_download_url(&origin, "file:///tmp/secret").is_err());

        let subpath_origin = Url::parse("https://openproject.example.com/team").unwrap();
        assert_eq!(
            resolve_download_url(&subpath_origin, "/api/v3/attachments/1/content")
                .unwrap()
                .as_str(),
            "https://openproject.example.com/team/api/v3/attachments/1/content"
        );
    }

    #[test]
    fn cross_origin_download_redirect_does_not_forward_authorization() {
        let external = TcpListener::bind("127.0.0.1:0").unwrap();
        let external_address = external.local_addr().unwrap();
        let (external_tx, external_rx) = mpsc::channel();
        let external_thread = std::thread::spawn(move || {
            let (mut stream, _) = external.accept().unwrap();
            external_tx.send(read_headers(&mut stream)).unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nproof")
                .unwrap();
        });

        let origin = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin_address = origin.local_addr().unwrap();
        let (origin_tx, origin_rx) = mpsc::channel();
        let origin_thread = std::thread::spawn(move || {
            let (mut stream, _) = origin.accept().unwrap();
            origin_tx.send(read_headers(&mut stream)).unwrap();
            write!(
                stream,
                "HTTP/1.1 302 Found\r\nLocation: http://{external_address}/proof\r\nContent-Length: 0\r\n\r\n"
            )
            .unwrap();
        });

        let client = OpenProjectClient::new(
            format!("http://{origin_address}"),
            "secret-token".to_owned(),
        )
        .unwrap();
        let mut downloaded = Vec::new();
        assert_eq!(client.download("/download", &mut downloaded).unwrap(), 5);
        assert_eq!(downloaded, b"proof");

        let origin_request = origin_rx.recv().unwrap().to_ascii_lowercase();
        let external_request = external_rx.recv().unwrap().to_ascii_lowercase();
        assert!(origin_request.contains("authorization: bearer secret-token"));
        assert!(!external_request.contains("authorization:"));
        origin_thread.join().unwrap();
        external_thread.join().unwrap();
    }
}
