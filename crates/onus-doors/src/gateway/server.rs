//! Git smart HTTP for the gateway. Each task's mirror is served at
//! `/<task>.git`; the token is the HTTP password (any user name). The
//! protocol itself is `git http-backend`, run as a CGI program per request,
//! so the gateway implements no git wire format.

use std::convert::Infallible;
use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;

use anyhow::{Context, Result};
use http_body_util::{BodyExt, Full};
use hyper::body::{Bytes, Incoming};
use hyper::{Request, Response, StatusCode};

use super::{Gateway, now};
use crate::audit::{AuditLog, Record};

/// What the server needs besides the gateway's settings.
#[derive(Debug, Clone)]
pub struct Server {
    pub gateway: Gateway,
    /// The Onus program the mirrors' hooks run.
    pub onus: PathBuf,
}

/// Serves until the process stops. `ready` gets the bound address.
pub fn serve(server: Server, addr: SocketAddr, ready: impl FnOnce(SocketAddr)) -> Result<()> {
    server.gateway.save()?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let server = Arc::new(server);
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::bind(addr).await?;
        ready(listener.local_addr()?);
        loop {
            let (stream, _) = listener.accept().await?;
            let server = server.clone();
            tokio::spawn(async move {
                let io = hyper_util::rt::TokioIo::new(stream);
                let service = hyper::service::service_fn(move |req| {
                    let server = server.clone();
                    async move { Ok::<_, Infallible>(server.handle(req).await) }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(io, service)
                    .await;
            });
        }
    })
}

fn text(status: StatusCode, body: impl Into<String>) -> Response<Full<Bytes>> {
    let mut r = Response::new(Full::new(Bytes::from(body.into())));
    *r.status_mut() = status;
    r.headers_mut().insert(
        "content-type",
        hyper::header::HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    r
}

fn unauthorized(why: &str) -> Response<Full<Bytes>> {
    let mut r = text(StatusCode::UNAUTHORIZED, format!("onus gateway: {why}\n"));
    r.headers_mut().insert(
        "www-authenticate",
        hyper::header::HeaderValue::from_static("Basic realm=\"onus gateway\""),
    );
    r
}

/// The token from `Authorization: Basic base64(user:token)`.
fn basic_password(req: &Request<Incoming>) -> Option<String> {
    let value = req.headers().get("authorization")?.to_str().ok()?;
    let encoded = value.strip_prefix("Basic ")?;
    let decoded = base64_decode(encoded.trim())?;
    let decoded = String::from_utf8(decoded).ok()?;
    let (_, password) = decoded.split_once(':')?;
    Some(password.to_string())
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let mut buf = 0u32;
    let mut bits = 0;
    for c in s.bytes().filter(|c| *c != b'=') {
        let v = ALPHABET.iter().position(|a| *a == c)? as u32;
        buf = (buf << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

impl Server {
    async fn handle(&self, req: Request<Incoming>) -> Response<Full<Bytes>> {
        match self.route(req).await {
            Ok(r) => r,
            Err(e) => text(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("onus gateway: {e:#}\n"),
            ),
        }
    }

    async fn route(&self, req: Request<Incoming>) -> Result<Response<Full<Bytes>>> {
        let path = req.uri().path().to_string();
        let Some((task, rest)) = path
            .trim_start_matches('/')
            .split_once(".git")
            .map(|(t, r)| (t.to_string(), r.to_string()))
        else {
            return Ok(text(
                StatusCode::NOT_FOUND,
                "onus gateway: clone /<task>.git\n",
            ));
        };
        let Some(raw) = basic_password(&req) else {
            return Ok(unauthorized("use your task token as the password"));
        };
        let token = match self.gateway.verify(&raw) {
            Ok(t) => t,
            Err(_) => return Ok(unauthorized("the token is not valid")),
        };
        if token.task != task {
            return Ok(text(
                StatusCode::FORBIDDEN,
                format!("onus gateway: this token is for task `{}`\n", token.task),
            ));
        }
        if now() > token.expires {
            return Ok(unauthorized("the token has expired"));
        }
        let gateway = self.gateway.clone();
        let onus = self.onus.clone();
        let raw_token = raw.clone();
        let fresh = !gateway.task_state_path(&task).exists();
        tokio::task::spawn_blocking(move || gateway.ensure_mirror(&token, &raw_token, &onus))
            .await??;
        if fresh {
            AuditLog::new(self.gateway.audit_path()).append(
                Record {
                    actor: task.clone(),
                    action: "mirror".into(),
                    subject: format!("{task}.git"),
                    decision: "created".into(),
                    reason: "first request with this task's token".into(),
                    details: serde_json::Value::Null,
                },
                now(),
            )?;
        }
        self.cgi(req, &task, &rest).await
    }

    /// Runs `git http-backend` for the request.
    async fn cgi(
        &self,
        req: Request<Incoming>,
        task: &str,
        rest: &str,
    ) -> Result<Response<Full<Bytes>>> {
        let method = req.method().to_string();
        let query = req.uri().query().unwrap_or("").to_string();
        let header = |name: &str| {
            req.headers()
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let content_type = header("content-type").unwrap_or_default();
        let encoding = header("content-encoding");
        let protocol = header("git-protocol");
        let body = req.into_body().collect().await?.to_bytes();
        let mut cmd = Command::new("git");
        cmd.arg("http-backend")
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("GIT_PROJECT_ROOT", &self.gateway.state)
            .env("GIT_HTTP_EXPORT_ALL", "1")
            .env("PATH_INFO", format!("/{task}.git{rest}"))
            .env("QUERY_STRING", query)
            .env("REQUEST_METHOD", method)
            .env("CONTENT_TYPE", content_type)
            .env("CONTENT_LENGTH", body.len().to_string())
            .env("REMOTE_USER", task)
            .env("REMOTE_ADDR", "127.0.0.1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(home) = std::env::var_os("HOME") {
            cmd.env("HOME", home);
        }
        if let Some(e) = encoding {
            cmd.env("HTTP_CONTENT_ENCODING", e);
        }
        if let Some(p) = protocol {
            cmd.env("GIT_PROTOCOL", p);
        }
        let output = tokio::task::spawn_blocking(move || -> Result<std::process::Output> {
            let mut child = cmd.spawn().context("cannot run `git http-backend`")?;
            child
                .stdin
                .take()
                .context("git http-backend stdin")?
                .write_all(&body)?;
            Ok(child.wait_with_output()?)
        })
        .await??;
        // CGI output: headers, a blank line, the body.
        let out = output.stdout;
        let split = out
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map(|i| (i, 4))
            .or_else(|| out.windows(2).position(|w| w == b"\n\n").map(|i| (i, 2)));
        let Some((at, sep)) = split else {
            return Ok(text(
                StatusCode::BAD_GATEWAY,
                format!(
                    "onus gateway: git http-backend failed: {}\n",
                    String::from_utf8_lossy(&output.stderr)
                ),
            ));
        };
        let mut response = Response::new(Full::new(Bytes::copy_from_slice(&out[at + sep..])));
        for line in String::from_utf8_lossy(&out[..at]).lines() {
            let Some((k, v)) = line.split_once(':') else {
                continue;
            };
            let (k, v) = (k.trim(), v.trim());
            if k.eq_ignore_ascii_case("status") {
                let code = v.split_whitespace().next().and_then(|c| c.parse().ok());
                if let Some(code) = code.and_then(|c: u16| StatusCode::from_u16(c).ok()) {
                    *response.status_mut() = code;
                }
            } else if let (Ok(name), Ok(value)) = (
                hyper::header::HeaderName::from_bytes(k.as_bytes()),
                hyper::header::HeaderValue::from_str(v),
            ) {
                response.headers_mut().append(name, value);
            }
        }
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn decodes_basic_auth() {
        assert_eq!(
            super::base64_decode("eDpzZWNyZXQ=").unwrap(),
            b"x:secret".to_vec()
        );
    }
}
