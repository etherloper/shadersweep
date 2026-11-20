//! Minimal HTTPS GET over WinHTTP. Using the OS stack means no bundled TLS library, and
//! requests respect the Windows certificate store and system proxy settings.

use std::ffi::c_void;
use std::io::{self, Read};
use std::{mem, ptr};

use windows_sys::Win32::Networking::WinHttp::{
    WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_FLAG_SECURE,
    WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen,
    WinHttpOpenRequest, WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest,
    WinHttpSetTimeouts,
};

const RESOLVE_TIMEOUT_MS: i32 = 10_000;
const CONNECT_TIMEOUT_MS: i32 = 10_000;
const TRANSFER_TIMEOUT_MS: i32 = 30_000;

#[derive(Debug, PartialEq, Eq)]
struct Target {
    secure: bool,
    host: String,
    port: u16,
    path: String,
}

/// Plain HTTP is only accepted for loopback addresses (a local test server).
fn parse_url(url: &str) -> io::Result<Target> {
    let invalid = |why: &str| io::Error::new(io::ErrorKind::InvalidInput, format!("{why}: {url}"));
    let (secure, rest) = if let Some(rest) = url.strip_prefix("https://") {
        (true, rest)
    } else if let Some(rest) = url.strip_prefix("http://") {
        (false, rest)
    } else {
        return Err(invalid("unsupported URL"));
    };
    let (authority, path) = rest.split_once('/').map_or((rest, "/".to_owned()), |(a, p)| (a, format!("/{p}")));
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host, port.parse::<u16>().map_err(|_| invalid("bad port"))?),
        None => (authority, if secure { 443 } else { 80 }),
    };
    if host.is_empty() {
        return Err(invalid("missing host"));
    }
    let loopback = matches!(host, "localhost" | "127.0.0.1" | "[::1]");
    if !secure && !loopback {
        return Err(invalid("refusing unencrypted connection"));
    }
    Ok(Target { secure, host: host.to_owned(), port, path })
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// Turns WinHTTP's numeric errors into something a person can act on.
fn last_error(context: &str) -> io::Error {
    let os = io::Error::last_os_error();
    let reason = match os.raw_os_error() {
        Some(12002) => "the server took too long to respond".to_owned(),
        Some(12007) => "the server name could not be resolved (are you online?)".to_owned(),
        Some(12029) | Some(12030) => "could not connect to the server".to_owned(),
        Some(12175) | Some(12045) | Some(12057) => "the server's security certificate was rejected".to_owned(),
        Some(code) => format!("network error {code}"),
        None => os.to_string(),
    };
    io::Error::other(format!("{context}: {reason}"))
}

struct Handle(*mut c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle came from WinHTTP and is closed exactly once.
        unsafe { WinHttpCloseHandle(self.0) };
    }
}

/// An open response whose body can be streamed with [`Read`].
pub struct Response {
    pub status: u16,
    // Field order matters: the request must close before its connection and session.
    request: Handle,
    _connection: Handle,
    _session: Handle,
}

impl Read for Response {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut read = 0u32;
        let len = buf.len().min(u32::MAX as usize) as u32;
        // SAFETY: `buf` is valid for `len` bytes and `read` receives the count.
        let ok = unsafe { WinHttpReadData(self.request.0, buf.as_mut_ptr().cast(), len, &mut read) };
        if ok == 0 {
            return Err(last_error("download interrupted"));
        }
        Ok(read as usize)
    }
}

fn query_number(request: &Handle, info: u32) -> Option<u64> {
    let mut value = 0u32;
    let mut size = mem::size_of::<u32>() as u32;
    // SAFETY: a u32 out-parameter sized correctly for FLAG_NUMBER queries.
    let ok = unsafe {
        WinHttpQueryHeaders(
            request.0,
            info | WINHTTP_QUERY_FLAG_NUMBER,
            ptr::null(),
            (&mut value as *mut u32).cast(),
            &mut size,
            ptr::null_mut(),
        )
    };
    (ok != 0).then_some(u64::from(value))
}

/// Sends a GET request and returns once headers arrive. Redirects are followed.
pub fn get(url: &str, user_agent: &str, accept: &str) -> io::Result<Response> {
    let target = parse_url(url)?;
    // SAFETY: every pointer passed is a NUL-terminated buffer that outlives the call, and
    // each returned handle is wrapped immediately so it is closed on all paths.
    unsafe {
        let agent = wide(user_agent);
        let session = WinHttpOpen(agent.as_ptr(), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, ptr::null(), ptr::null(), 0);
        if session.is_null() {
            return Err(last_error("could not start a network session"));
        }
        let session = Handle(session);
        WinHttpSetTimeouts(session.0, RESOLVE_TIMEOUT_MS, CONNECT_TIMEOUT_MS, TRANSFER_TIMEOUT_MS, TRANSFER_TIMEOUT_MS);

        let host = wide(&target.host);
        let connection = WinHttpConnect(session.0, host.as_ptr(), target.port, 0);
        if connection.is_null() {
            return Err(last_error("could not connect"));
        }
        let connection = Handle(connection);

        let verb = wide("GET");
        let path = wide(&target.path);
        let flags = if target.secure { WINHTTP_FLAG_SECURE } else { 0 };
        let request = WinHttpOpenRequest(
            connection.0,
            verb.as_ptr(),
            path.as_ptr(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            flags,
        );
        if request.is_null() {
            return Err(last_error("could not create the request"));
        }
        let request = Handle(request);

        let headers = wide(&format!("Accept: {accept}\r\n"));
        let sent = WinHttpSendRequest(request.0, headers.as_ptr(), u32::MAX, ptr::null(), 0, 0, 0);
        if sent == 0 || WinHttpReceiveResponse(request.0, ptr::null_mut()) == 0 {
            return Err(last_error("request failed"));
        }

        let status = query_number(&request, WINHTTP_QUERY_STATUS_CODE).unwrap_or(0) as u16;
        Ok(Response { status, request, _connection: connection, _session: session })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_urls() {
        assert_eq!(
            parse_url("https://api.github.com/repos/a/b/releases/latest").unwrap(),
            Target {
                secure: true,
                host: "api.github.com".into(),
                port: 443,
                path: "/repos/a/b/releases/latest".into()
            }
        );
        assert_eq!(parse_url("http://127.0.0.1:8765/latest").unwrap().port, 8765);
        assert_eq!(parse_url("https://example.com").unwrap().path, "/");
    }

    #[test]
    fn rejects_unencrypted_remote_and_odd_urls() {
        assert!(parse_url("http://example.com/file.exe").is_err());
        assert!(parse_url("ftp://example.com/").is_err());
        assert!(parse_url("https://:443/").is_err());
        assert!(parse_url("https://host:notaport/").is_err());
    }
}
