use anyhow::{anyhow, Context, Result};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Mutex, OnceLock},
    time::Duration,
};
use tokio::sync::{watch, Mutex as AsyncMutex};

type UploadChunk = std::result::Result<Vec<u8>, String>;

#[flutter_rust_bridge::frb(opaque)]
pub struct HttpUpload {
    sender: Mutex<Option<tokio::sync::mpsc::Sender<UploadChunk>>>,
    receiver: Mutex<Option<tokio::sync::mpsc::Receiver<UploadChunk>>>,
}

impl HttpUpload {
    #[flutter_rust_bridge::frb(sync)]
    pub fn new() -> Self {
        let (sender, receiver) = tokio::sync::mpsc::channel(8);
        Self {
            sender: Mutex::new(Some(sender)),
            receiver: Mutex::new(Some(receiver)),
        }
    }

    pub async fn write(&self, bytes: Vec<u8>) -> Result<()> {
        let sender = self
            .sender
            .lock()
            .map_err(|_| anyhow!("upload lock poisoned"))?
            .clone()
            .context("upload closed")?;
        sender
            .send(Ok(bytes))
            .await
            .map_err(|_| anyhow!("upload request closed"))
    }

    pub async fn fail(&self, message: String) -> Result<()> {
        let sender = self
            .sender
            .lock()
            .map_err(|_| anyhow!("upload lock poisoned"))?
            .take();
        if let Some(sender) = sender {
            let _ = sender.send(Err(message)).await;
        }
        Ok(())
    }

    #[flutter_rust_bridge::frb(sync)]
    pub fn finish(&self) -> Result<()> {
        self.sender
            .lock()
            .map_err(|_| anyhow!("upload lock poisoned"))?
            .take();
        Ok(())
    }

    #[flutter_rust_bridge::frb(sync)]
    pub fn abort(&self) -> Result<()> {
        self.finish()?;
        self.receiver
            .lock()
            .map_err(|_| anyhow!("upload lock poisoned"))?
            .take();
        Ok(())
    }

    #[flutter_rust_bridge::frb(ignore)]
    fn body(&self) -> Result<reqwest::Body> {
        let receiver = self
            .receiver
            .lock()
            .map_err(|_| anyhow!("upload lock poisoned"))?
            .take()
            .context("upload already consumed")?;
        let stream = futures_util::stream::unfold(receiver, |mut receiver| async {
            receiver
                .recv()
                .await
                .map(|chunk| (chunk.map_err(std::io::Error::other), receiver))
        });
        Ok(reqwest::Body::wrap_stream(stream))
    }
}

impl Default for HttpUpload {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
pub struct HttpHeader {
    pub name: String,
    pub value: String,
}

pub struct HttpOptions {
    pub proxy: Option<String>,
    pub dns_overrides: HashMap<String, String>,
    pub tls_sni: bool,
    pub verify_certificates: bool,
    pub connect_timeout_ms: u32,
    pub receive_timeout_ms: u32,
    pub max_redirects: u32,
}

#[flutter_rust_bridge::frb(opaque)]
pub struct HttpCancellation {
    sender: watch::Sender<bool>,
}

impl HttpCancellation {
    #[flutter_rust_bridge::frb(sync)]
    pub fn new() -> Self {
        Self {
            sender: watch::channel(false).0,
        }
    }

    #[flutter_rust_bridge::frb(sync)]
    pub fn cancel(&self) {
        self.sender.send_replace(true);
    }

    #[flutter_rust_bridge::frb(ignore)]
    pub(crate) async fn cancelled(&self) {
        let mut receiver = self.sender.subscribe();
        if !*receiver.borrow_and_update() {
            let _ = receiver.changed().await;
        }
    }
}

impl Default for HttpCancellation {
    fn default() -> Self {
        Self::new()
    }
}

#[flutter_rust_bridge::frb(opaque)]
pub struct HttpResponse {
    response: AsyncMutex<Option<reqwest::Response>>,
    headers: Vec<HttpHeader>,
    status: u16,
    url: String,
}

impl HttpResponse {
    #[flutter_rust_bridge::frb(sync)]
    pub fn status(&self) -> u16 {
        self.status
    }

    #[flutter_rust_bridge::frb(sync)]
    pub fn headers(&self) -> Vec<HttpHeader> {
        self.headers.clone()
    }

    #[flutter_rust_bridge::frb(sync)]
    pub fn url(&self) -> String {
        self.url.clone()
    }

    // Pull-based streaming provides backpressure: Dart requests the next chunk
    // only after its consumer has processed the previous chunk.
    pub async fn next_chunk(&self, cancellation: &HttpCancellation) -> Result<Option<Vec<u8>>> {
        let mut guard = self.response.lock().await;
        let Some(response) = guard.as_mut() else {
            return Ok(None);
        };
        let result = tokio::select! {
            _ = cancellation.cancelled() => Err(anyhow!("Request cancelled")),
            chunk = response.chunk() => chunk.map(|value| value.map(|bytes| bytes.to_vec())).map_err(Into::into),
        };
        if !matches!(result, Ok(Some(_))) {
            guard.take();
        }
        result
    }

    pub async fn close(&self) {
        self.response.lock().await.take();
    }
}

static CLIENTS: OnceLock<Mutex<HashMap<String, reqwest::Client>>> = OnceLock::new();

pub(crate) fn client(options: &HttpOptions) -> Result<reqwest::Client> {
    let mut dns: Vec<_> = options.dns_overrides.iter().collect();
    dns.sort();
    let key = format!(
        "{:?}|{:?}|{}|{}|{}|{}|{}",
        options.proxy,
        dns,
        options.tls_sni,
        options.verify_certificates,
        options.connect_timeout_ms,
        options.receive_timeout_ms,
        options.max_redirects
    );
    let mut clients = CLIENTS
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| anyhow!("HTTP pool lock poisoned"))?;
    if let Some(client) = clients.get(&key) {
        return Ok(client.clone());
    }
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .tls_sni(options.tls_sni)
        .danger_accept_invalid_certs(!options.verify_certificates)
        .redirect(if options.max_redirects == 0 {
            reqwest::redirect::Policy::none()
        } else {
            reqwest::redirect::Policy::limited(options.max_redirects as usize)
        })
        .connect_timeout(Duration::from_millis(u64::from(options.connect_timeout_ms)))
        .read_timeout(Duration::from_millis(u64::from(options.receive_timeout_ms)))
        .pool_idle_timeout(Duration::from_secs(60));
    if let Some(proxy) = &options.proxy {
        let proxy = if proxy.contains("://") {
            proxy.clone()
        } else {
            format!("http://{proxy}")
        };
        builder = builder.proxy(reqwest::Proxy::all(proxy)?);
    }
    for (host, address) in &options.dns_overrides {
        let ip: IpAddr = address
            .parse()
            .with_context(|| format!("Invalid DNS override for {host}"))?;
        builder = builder.resolve(host, SocketAddr::new(ip, 0));
    }
    let client = builder.build()?;
    // Bound the number of pools when settings change repeatedly.
    if clients.len() >= 8 {
        clients.clear();
    }
    clients.insert(key, client.clone());
    Ok(client)
}

#[allow(clippy::too_many_arguments)]
pub async fn http_request(
    url: String,
    method: String,
    headers: Vec<HttpHeader>,
    body: Option<Vec<u8>>,
    upload: &HttpUpload,
    has_upload: bool,
    options: HttpOptions,
    cancellation: &HttpCancellation,
) -> Result<HttpResponse> {
    let mut request = client(&options)?.request(method.parse()?, url);
    for header in headers {
        request = request.header(header.name, header.value);
    }
    if has_upload {
        request = request.body(upload.body()?);
    } else if let Some(body) = body {
        request = request.body(body);
    }
    let response = tokio::select! {
        _ = cancellation.cancelled() => return Err(anyhow!("Request cancelled")),
        response = request.send() => response?,
    };
    let headers = response
        .headers()
        .iter()
        .map(|(name, value)| HttpHeader {
            name: name.to_string(),
            value: String::from_utf8_lossy(value.as_bytes()).into_owned(),
        })
        .collect();
    Ok(HttpResponse {
        status: response.status().as_u16(),
        url: response.url().to_string(),
        headers,
        response: AsyncMutex::new(Some(response)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn options() -> HttpOptions {
        HttpOptions {
            proxy: None,
            dns_overrides: HashMap::new(),
            tls_sni: true,
            verify_certificates: true,
            connect_timeout_ms: 5000,
            receive_timeout_ms: 5000,
            max_redirects: 5,
        }
    }

    #[tokio::test]
    async fn streams_binary_response_and_preserves_duplicate_headers() -> Result<()> {
        let server = std::net::TcpListener::bind("127.0.0.1:0")?;
        let url = format!("http://{}/", server.local_addr()?);
        let thread = std::thread::spawn(move || {
            let (mut socket, _) = server.accept().unwrap();
            let mut buffer = [0; 4096];
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") {
                let read = socket.read(&mut buffer).unwrap();
                assert!(read > 0, "client closed before sending headers");
                request.extend_from_slice(&buffer[..read]);
            }
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nSet-Cookie: a=1\r\nSet-Cookie: b=2\r\nConnection: close\r\n\r\n\x00\x7f\xff").unwrap();
        });
        let cancellation = HttpCancellation::new();
        let response = http_request(
            url,
            "GET".into(),
            vec![],
            None,
            &HttpUpload::new(),
            false,
            options(),
            &cancellation,
        )
        .await?;
        assert_eq!(response.status(), 200);
        assert_eq!(
            response
                .headers()
                .iter()
                .filter(|h| h.name == "set-cookie")
                .count(),
            2
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response.next_chunk(&cancellation).await? {
            bytes.extend(chunk);
        }
        assert_eq!(bytes, vec![0, 127, 255]);
        thread.join().unwrap();
        Ok(())
    }

    #[tokio::test]
    async fn cancellation_is_not_lost_before_request_starts() {
        let cancellation = HttpCancellation::new();
        cancellation.cancel();
        assert!(http_request(
            "http://127.0.0.1:1/".into(),
            "GET".into(),
            vec![],
            None,
            &HttpUpload::new(),
            false,
            options(),
            &cancellation
        )
        .await
        .is_err());
    }
}
