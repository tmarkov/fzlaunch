use std::pin::Pin;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

const PREVIEW_OUTPUT_LIMIT: usize = 64 * 1024;
const PREVIEW_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Preview {
    Unavailable,
    Loading,
    Ready(String),
}

pub struct PreviewOutput {
    pub command: String,
    pub preview: Preview,
}

pub struct PreviewRunner {
    sender: tokio::sync::mpsc::Sender<PreviewOutput>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl PreviewRunner {
    pub fn new(sender: tokio::sync::mpsc::Sender<PreviewOutput>) -> Self {
        Self { sender, task: None }
    }

    pub fn start(&mut self, command: String) {
        self.abort();

        let sender = self.sender.clone();
        self.task = Some(tokio::spawn(async move {
            let output = command_output(command.clone()).await;
            let _ = sender
                .send(PreviewOutput {
                    command,
                    preview: Preview::Ready(output),
                })
                .await;
        }));
    }

    pub fn abort(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl Drop for PreviewRunner {
    fn drop(&mut self) {
        self.abort();
    }
}

async fn command_output(command: String) -> String {
    let output = tokio::time::timeout(PREVIEW_TIMEOUT, command_output_inner(command)).await;

    match output {
        Ok(output) => output,
        Err(_) => "preview timed out".to_string(),
    }
}

async fn command_output_inner(command: String) -> String {
    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(command)
        .env("MANPAGER", "cat")
        .env("PAGER", "cat")
        .env("TERM", "dumb")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return "preview failed".to_string(),
    };

    let Some(stdout) = child.stdout.take() else {
        return "preview failed".to_string();
    };
    let Some(stderr) = child.stderr.take() else {
        return "preview failed".to_string();
    };

    let stdout = read_limited(Box::pin(stdout));
    let stderr = read_limited(Box::pin(stderr));
    let status = child.wait();
    let (stdout, stderr, status) = tokio::join!(stdout, stderr, status);

    if status.is_err() {
        return "preview failed".to_string();
    }

    let stdout = stdout.unwrap_or_default();
    let stderr = stderr.unwrap_or_default();
    let bytes = if stdout.is_empty() { stderr } else { stdout };
    let truncated = bytes.len() > PREVIEW_OUTPUT_LIMIT;
    let bytes = &bytes[..bytes.len().min(PREVIEW_OUTPUT_LIMIT)];
    let text = String::from_utf8_lossy(bytes);
    limit_output(text.trim_end(), truncated)
}

async fn read_limited(mut reader: Pin<Box<dyn AsyncRead + Send>>) -> std::io::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut buffer = [0; 8192];

    loop {
        let bytes_read = reader.read(&mut buffer).await?;
        if bytes_read == 0 {
            return Ok(output);
        }

        let remaining = PREVIEW_OUTPUT_LIMIT + 1 - output.len();
        output.extend_from_slice(&buffer[..bytes_read.min(remaining)]);

        if output.len() > PREVIEW_OUTPUT_LIMIT {
            return Ok(output);
        }
    }
}

fn limit_output(text: &str, truncated: bool) -> String {
    let mut output = text.to_string();
    if truncated {
        output.push_str("\n...");
    }
    if output.is_empty() {
        "no preview output".to_string()
    } else {
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn command_output_captures_stdout() {
        assert_eq!(
            command_output("printf 'hello preview'".to_string()).await,
            "hello preview"
        );
    }

    #[test]
    fn empty_output_has_placeholder() {
        assert_eq!(limit_output("", false), "no preview output");
    }
}
