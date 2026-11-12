use super::http::{client, HttpCancellation, HttpOptions};
use anyhow::{anyhow, ensure, Context, Result};
use futures_util::{stream, StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, Mutex as AsyncMutex};

#[derive(Clone)]
pub struct DownloadProgress {
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub bytes_per_second: u64,
    pub finished: bool,
}

#[flutter_rust_bridge::frb(opaque)]
pub struct FileDownload {
    cancellation: Arc<HttpCancellation>,
    updates: AsyncMutex<mpsc::Receiver<std::result::Result<DownloadProgress, String>>>,
    task: AsyncMutex<Option<tokio::task::JoinHandle<()>>>,
}

impl FileDownload {
    pub async fn start(
        url: String,
        path: String,
        concurrency: u32,
        options: HttpOptions,
    ) -> Result<Self> {
        ensure!(
            (1..=32).contains(&concurrency),
            "Download concurrency must be between 1 and 32"
        );
        let http = client(&options)?;
        let cancellation = Arc::new(HttpCancellation::new());
        let cancel = cancellation.clone();
        let (sender, updates) = mpsc::channel(16);
        let task = tokio::spawn(async move {
            let progress = Arc::new(Mutex::new(DownloadProgress {
                downloaded_bytes: 0,
                total_bytes: 0,
                bytes_per_second: 0,
                finished: false,
            }));
            let work = download(
                http,
                &url,
                Path::new(&path),
                concurrency as usize,
                progress.clone(),
            );
            tokio::pin!(work);
            let mut ticker = tokio::time::interval(Duration::from_secs(1));
            let mut last = 0;
            let mut last_time = Instant::now();
            loop {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => break,
                    result = &mut work => {
                        let status = result.map(|()| {
                            let mut value = progress.lock().unwrap_or_else(|e| e.into_inner()).clone();
                            value.finished = true;
                            value.bytes_per_second = 0;
                            value
                        }).map_err(|e| format!("{e:#}"));
                        tokio::select! { _ = cancel.cancelled() => {}, _ = sender.send(status) => {} }
                        break;
                    }
                    _ = ticker.tick() => {
                        let mut status = progress.lock().unwrap_or_else(|e| e.into_inner()).clone();
                        let elapsed = last_time.elapsed().as_secs_f64().max(0.001);
                        status.bytes_per_second = (status.downloaded_bytes.saturating_sub(last) as f64 / elapsed) as u64;
                        last = status.downloaded_bytes;
                        last_time = Instant::now();
                        // Progress is advisory; a slow UI must not block the downloader.
                        if sender.is_closed() { break; }
                        let _ = sender.try_send(Ok(status));
                    }
                }
            }
        });
        Ok(Self {
            cancellation,
            updates: AsyncMutex::new(updates),
            task: AsyncMutex::new(Some(task)),
        })
    }

    pub async fn next_status(&self) -> Result<Option<DownloadProgress>> {
        self.updates
            .lock()
            .await
            .recv()
            .await
            .transpose()
            .map_err(|e| anyhow!(e))
    }

    pub async fn stop(&self) -> Result<()> {
        self.cancellation.cancel();
        if let Some(task) = self.task.lock().await.take() {
            task.await?;
        }
        Ok(())
    }
}

impl Drop for FileDownload {
    fn drop(&mut self) {
        self.cancellation.cancel();
    }
}

#[derive(Clone, Debug)]
struct Block {
    start: u64,
    end: u64,
    downloaded: u64,
}

#[derive(Serialize, Deserialize, PartialEq, Eq)]
struct Identity {
    url: String,
    validator: Option<String>,
    size: u64,
}

struct Output {
    file: File,
    blocks: Vec<Block>,
    status: PathBuf,
    progress: Arc<Mutex<DownloadProgress>>,
}

impl Output {
    fn write(&mut self, index: usize, bytes: &[u8]) -> Result<()> {
        let block = &mut self.blocks[index];
        ensure!(
            block.downloaded + bytes.len() as u64 <= block.end - block.start,
            "Server sent too many range bytes"
        );
        self.file
            .seek(SeekFrom::Start(block.start + block.downloaded))?;
        self.file.write_all(bytes)?;
        self.file.sync_data()?;
        block.downloaded += bytes.len() as u64;
        let state = self
            .blocks
            .iter()
            .map(|b| format!("{}-{}-{}", b.start, b.end, b.downloaded))
            .collect::<Vec<_>>()
            .join("\n");
        atomic_write(&self.status, state.as_bytes())?;
        self.progress
            .lock()
            .map_err(|_| anyhow!("progress lock poisoned"))?
            .downloaded_bytes = self.blocks.iter().map(|b| b.downloaded).sum();
        Ok(())
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut temporary = tempfile::NamedTempFile::new_in(
        path.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new(".")),
    )?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path)?;
    Ok(())
}

fn parse_blocks(text: &str, total: u64) -> Option<Vec<Block>> {
    let mut blocks = Vec::new();
    let mut next = 0;
    for line in text.lines() {
        let values = line
            .split('-')
            .map(str::parse::<u64>)
            .collect::<std::result::Result<Vec<_>, _>>()
            .ok()?;
        if values.len() != 3
            || values[0] != next
            || values[1] <= values[0]
            || values[1] > total
            || values[2] > values[1] - values[0]
        {
            return None;
        }
        blocks.push(Block {
            start: values[0],
            end: values[1],
            downloaded: values[2],
        });
        next = values[1];
    }
    (next == total).then_some(blocks)
}

fn content_range(value: &str) -> Option<(u64, u64, u64)> {
    let value = value.strip_prefix("bytes ")?;
    let (range, total) = value.split_once('/')?;
    let (start, end) = range.split_once('-')?;
    Some((start.parse().ok()?, end.parse().ok()?, total.parse().ok()?))
}

fn validator(response: &reqwest::Response) -> Option<String> {
    response
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.starts_with("W/"))
        .or_else(|| {
            response
                .headers()
                .get("last-modified")
                .and_then(|v| v.to_str().ok())
        })
        .map(str::to_owned)
}

async fn download(
    http: reqwest::Client,
    url: &str,
    path: &Path,
    concurrency: usize,
    progress: Arc<Mutex<DownloadProgress>>,
) -> Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let status = PathBuf::from(format!("{}.download", path.display()));
    let meta = PathBuf::from(format!("{}.download.meta", path.display()));
    let mut probe = http
        .get(url)
        .header("Range", "bytes=0-0")
        .header("Accept-Encoding", "identity")
        .send()
        .await?
        .error_for_status()?;
    ensure!(
        probe
            .headers()
            .get("content-encoding")
            .is_none_or(|v| v == "identity"),
        "Encoded range response is unsupported"
    );
    if probe.status() != reqwest::StatusCode::PARTIAL_CONTENT {
        // Range-ignorant servers and unknown lengths are streamed from zero.
        // Invalidate range state before overwriting any previously downloaded bytes.
        if meta.exists() {
            std::fs::remove_file(&meta)?;
        }
        let mut file = File::create(path)?;
        let expected = probe.content_length();
        progress
            .lock()
            .map_err(|_| anyhow!("progress lock poisoned"))?
            .total_bytes = expected.unwrap_or(0);
        let mut count = 0;
        while let Some(bytes) = probe.chunk().await? {
            file.write_all(&bytes)?;
            count += bytes.len() as u64;
            progress
                .lock()
                .map_err(|_| anyhow!("progress lock poisoned"))?
                .downloaded_bytes = count;
        }
        ensure!(
            expected.is_none_or(|size| size == count),
            "Incomplete download"
        );
        file.sync_all()?;
        progress
            .lock()
            .map_err(|_| anyhow!("progress lock poisoned"))?
            .total_bytes = count;
    } else {
        let (start, end, size) = content_range(
            probe
                .headers()
                .get("content-range")
                .and_then(|v| v.to_str().ok())
                .context("Missing Content-Range")?,
        )
        .context("Invalid Content-Range")?;
        ensure!(
            start == 0 && end == 0 && size > 0,
            "Invalid range probe response"
        );
        let identity = Identity {
            url: url.into(),
            validator: validator(&probe),
            size,
        };
        drop(probe);
        let previous = std::fs::read(&meta)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Identity>(&bytes).ok());
        let resume = identity.validator.is_some()
            && previous.as_ref() == Some(&identity)
            && path.metadata().is_ok_and(|metadata| metadata.len() == size);
        let saved_blocks = if resume {
            std::fs::read_to_string(&status)
                .ok()
                .and_then(|text| parse_blocks(&text, size))
        } else {
            None
        };
        let blocks = saved_blocks.unwrap_or_else(|| {
            let chunk = if size > 1024 * 1024 * 1024 {
                64 * 1024 * 1024
            } else if size > 512 * 1024 * 1024 {
                32 * 1024 * 1024
            } else {
                16 * 1024 * 1024
            };
            (0..size)
                .step_by(chunk)
                .map(|start| Block {
                    start,
                    end: (start + chunk as u64).min(size),
                    downloaded: 0,
                })
                .collect()
        });
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        file.set_len(size)?;
        // Publish the matching block state before its identity, so interrupted
        // restarts can never associate stale offsets with a different resource.
        if meta.exists() {
            std::fs::remove_file(&meta)?;
        }
        let block_state = blocks
            .iter()
            .map(|b| format!("{}-{}-{}", b.start, b.end, b.downloaded))
            .collect::<Vec<_>>()
            .join("\n");
        atomic_write(&status, block_state.as_bytes())?;
        atomic_write(&meta, &serde_json::to_vec(&identity)?)?;
        {
            let mut state = progress
                .lock()
                .map_err(|_| anyhow!("progress lock poisoned"))?;
            state.total_bytes = size;
            state.downloaded_bytes = blocks.iter().map(|b| b.downloaded).sum();
        }
        let pending: Vec<_> = blocks
            .iter()
            .enumerate()
            .filter(|(_, b)| b.downloaded < b.end - b.start)
            .map(|(i, b)| (i, b.clone()))
            .collect();
        let output = Arc::new(Mutex::new(Output {
            file,
            blocks,
            status: status.clone(),
            progress: progress.clone(),
        }));
        stream::iter(pending)
            .map(|(index, block)| {
                let http = http.clone();
                let output = output.clone();
                let identity = &identity;
                async move {
                    let begin = block.start + block.downloaded;
                    let mut request = http
                        .get(url)
                        .header("Range", format!("bytes={begin}-{}", block.end - 1))
                        .header("Accept-Encoding", "identity");
                    if let Some(validator) = &identity.validator {
                        request = request.header("If-Range", validator);
                    }
                    let mut response = request.send().await?.error_for_status()?;
                    ensure!(
                        response.status() == reqwest::StatusCode::PARTIAL_CONTENT,
                        "Server did not honor range or file changed; retry to restart"
                    );
                    let range = response
                        .headers()
                        .get("content-range")
                        .and_then(|v| v.to_str().ok())
                        .and_then(content_range);
                    ensure!(
                        range == Some((begin, block.end - 1, size)),
                        "Unexpected Content-Range"
                    );
                    ensure!(
                        response
                            .headers()
                            .get("content-encoding")
                            .is_none_or(|v| v == "identity"),
                        "Encoded range response is unsupported"
                    );
                    let mut buffer = Vec::with_capacity(256 * 1024);
                    let mut received = 0;
                    while let Some(bytes) = response.chunk().await? {
                        received += bytes.len() as u64;
                        ensure!(received <= block.end - begin, "Excess range data");
                        buffer.extend_from_slice(&bytes);
                        if buffer.len() >= 256 * 1024 {
                            output
                                .lock()
                                .map_err(|_| anyhow!("download lock poisoned"))?
                                .write(index, &buffer)?;
                            buffer.clear();
                        }
                    }
                    ensure!(received == block.end - begin, "Incomplete range data");
                    if !buffer.is_empty() {
                        output
                            .lock()
                            .map_err(|_| anyhow!("download lock poisoned"))?
                            .write(index, &buffer)?;
                    }
                    Ok::<_, anyhow::Error>(())
                }
            })
            .buffer_unordered(concurrency)
            .try_collect::<Vec<_>>()
            .await?;
        output
            .lock()
            .map_err(|_| anyhow!("download lock poisoned"))?
            .file
            .sync_all()?;
    }
    for file in [&status, &meta] {
        match std::fs::remove_file(file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_legacy_resume_blocks() {
        assert!(parse_blocks("0-10-5\n10-20-10", 20).is_some());
        for invalid in [
            "0-10-11\n10-20-0",
            "0-10-0\n11-20-0",
            "0-10-0",
            "0-21-0",
            "bad",
        ] {
            assert!(parse_blocks(invalid, 20).is_none());
        }
        assert_eq!(content_range("bytes 3-9/10"), Some((3, 9, 10)));
        assert_eq!(content_range("bytes */10"), None);
    }
}
