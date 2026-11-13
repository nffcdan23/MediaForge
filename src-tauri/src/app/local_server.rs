//! A minimal HTTP server on the loopback interface that serves the files in one folder.
//!
//! Used by the self-test and the integration tests to exercise real downloads through yt-dlp without
//! contacting anything outside the computer. Never started during normal use.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

pub struct LocalServer {
    pub port: u16,
    running: Arc<AtomicBool>,
}

impl LocalServer {
    /// Serves `root` on 127.0.0.1 at a free port until dropped. `throttle` slows responses, for testing
    /// cancellation part-way through a transfer.
    pub fn start(root: &Path, throttle: Option<u64>) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let running = Arc::new(AtomicBool::new(true));
        let flag = Arc::clone(&running);
        let root = root.to_path_buf();
        thread::spawn(move || {
            while flag.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let root = root.clone();
                        thread::spawn(move || {
                            let _ = handle(stream, &root, throttle);
                        });
                    }
                    Err(_) => thread::sleep(Duration::from_millis(20)),
                }
            }
        });
        Ok(Self { port, running })
    }

    pub fn url(&self, name: &str) -> String {
        format!("http://127.0.0.1:{}/{}", self.port, name)
    }
}

impl Drop for LocalServer {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
    }
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase().as_str() {
        "wav" => "audio/wav",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mpd" => "application/dash+xml",
        "xml" | "rss" => "application/rss+xml",
        "html" => "text/html; charset=utf-8",
        "webp" => "image/webp",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        _ => "application/octet-stream",
    }
}

fn handle(stream: TcpStream, root: &Path, throttle: Option<u64>) -> std::io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let mut range_start: Option<u64> = None;
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header)? == 0 || header == "\r\n" || header == "\n" {
            break;
        }
        let lower = header.to_ascii_lowercase();
        if let Some(value) = lower.strip_prefix("range: bytes=") {
            range_start = value.trim().split('-').next().and_then(|s| s.parse().ok());
        }
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("/");
    let raw_name = target.trim_start_matches('/').split(['?', '#']).next().unwrap_or("");
    let name = percent_decode(raw_name);
    let name = name.as_str();
    let mut stream = stream;
    // Only plain file names: no folders, no escaping the root.
    let path: PathBuf = root.join(name);
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.contains("..") || !path.is_file() {
        stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\nConnection: close\r\n\r\nNot Found")?;
        return Ok(());
    }
    let data = std::fs::read(&path)?;
    let start = range_start.unwrap_or(0).min(data.len() as u64) as usize;
    let body = &data[start..];
    let status = if range_start.is_some() { "206 Partial Content" } else { "200 OK" };
    let mut headers = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n",
        content_type(&path),
        body.len()
    );
    if range_start.is_some() {
        headers.push_str(&format!("Content-Range: bytes {start}-{}/{}\r\n", data.len().saturating_sub(1), data.len()));
    }
    headers.push_str("\r\n");
    stream.write_all(headers.as_bytes())?;
    if method == "HEAD" {
        return Ok(());
    }
    match throttle {
        None => stream.write_all(body)?,
        Some(bytes_per_second) => {
            let chunk = (bytes_per_second / 10).max(1) as usize;
            for piece in body.chunks(chunk) {
                stream.write_all(piece)?;
                thread::sleep(Duration::from_millis(100));
            }
        }
    }
    let _ = stream.flush();
    let mut sink = [0u8; 1];
    let _ = stream.read(&mut sink);
    Ok(())
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = |b: u8| (b as char).to_digit(16);
            if let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2])) {
                out.push((high * 16 + low) as u8);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A sine tone as a 16-bit stereo WAV file, so tests have real audio without generating it with FFmpeg.
pub fn tone_wav(seconds: f64, sample_rate: u32, frequency: f64) -> Vec<u8> {
    let frames = (seconds * sample_rate as f64) as u32;
    let channels = 2u16;
    let data_len = frames * channels as u32 * 2;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * channels as u32 * 2).to_le_bytes());
    out.extend_from_slice(&(channels * 2).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for frame in 0..frames {
        let t = frame as f64 / sample_rate as f64;
        let sample = ((t * frequency * std::f64::consts::TAU).sin() * 0.4 * i16::MAX as f64) as i16;
        for _ in 0..channels {
            out.extend_from_slice(&sample.to_le_bytes());
        }
    }
    out
}
