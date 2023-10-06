use crate::container_io::Pipe;
use anyhow::{Context, Result};
use getset::{CopyGetters, Getters, Setters};
use serde_json::json;
use std::{
    marker::Unpin,
    path::{Path, PathBuf},
};
use tokio::{
    fs::{File, OpenOptions},
    io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter},
};
use tracing::debug;

#[derive(Debug, CopyGetters, Getters, Setters)]
pub struct JsonLogger {
    #[getset(get)]
    path: PathBuf,

    #[getset(set)]
    file: Option<BufWriter<File>>,

    #[getset(get_copy)]
    max_log_size: Option<usize>,

    #[getset(get_copy, set)]
    bytes_written: usize,
}

impl JsonLogger {
    const ERR_UNINITIALIZED: &'static str = "logger not initialized";

    pub fn new<T: AsRef<Path>>(path: T, max_log_size: Option<usize>) -> Result<JsonLogger> {
        Ok(Self {
            path: path.as_ref().into(),
            file: None,
            max_log_size,
            bytes_written: 0,
        })
    }

    pub async fn init(&mut self) -> Result<()> {
        debug!("Initializing JSON logger in path {}", self.path().display());
        self.set_file(Self::open(self.path()).await?.into());
        Ok(())
    }

    pub async fn write<T>(&mut self, pipe: Pipe, bytes: T) -> Result<()>
    where
        T: AsyncBufRead + Unpin,
    {
        let mut reader = BufReader::new(bytes);
        let mut line_buf = Vec::new();

        while reader.read_until(b'\n', &mut line_buf).await? > 0 {
            let log_entry = json!({
                "timestamp": format!("{:?}", std::time::SystemTime::now()),
                "pipe": match pipe {
                    Pipe::StdOut => "stdout",
                    Pipe::StdErr => "stderr",
                },
                "message": String::from_utf8_lossy(&line_buf).trim().to_string()
            });

            let log_str = log_entry.to_string();
            let bytes = log_str.as_bytes();
            self.bytes_written += bytes.len();

            if let Some(max_size) = self.max_log_size {
                if self.bytes_written > max_size {
                    self.reopen().await?;
                    self.bytes_written = 0;
                }
            }

            let file = self.file.as_mut().context(Self::ERR_UNINITIALIZED)?;
            file.write_all(bytes).await?;
            file.write_all(b"\n").await?; // Newline for each JSON log entry

            line_buf.clear();
        }

        Ok(())
    }

    pub async fn reopen(&mut self) -> Result<()> {
        debug!("Reopen JSON log {}", self.path().display());
        self.file
            .as_mut()
            .context(Self::ERR_UNINITIALIZED)?
            .get_ref()
            .sync_all()
            .await?;
        self.init().await
    }

    pub async fn flush(&mut self) -> Result<()> {
        self.file
            .as_mut()
            .context(Self::ERR_UNINITIALIZED)?
            .flush()
            .await
            .context("flush file writer")
    }

    async fn open<T: AsRef<Path>>(path: T) -> Result<BufWriter<File>> {
        Ok(BufWriter::new(
            OpenOptions::new()
                .create(true)
                .read(true)
                .truncate(true)
                .write(true)
                .open(&path)
                .await
                .context(format!("open log file path '{}'", path.as_ref().display()))?,
        ))
    }
}



mod tests {
    use super::*;
    use serde_json::Value;
    use std::fs;
    use tempfile::NamedTempFile;
    use futures::io::Cursor;

    #[tokio::test]
    async fn write_stdout_success() -> Result<(), Box<dyn std::error::Error>> {
        let message = "this is a line\n";
        let error_message = "and another line\n";

        let file = NamedTempFile::new()?;
        let path = file.path();
        let mut logger = JsonLogger::new(path, None);
        logger.init().await?;

        // Use Cursor to simulate AsyncBufRead for our test strings
        // Assuming JsonLogger has an async write method that takes a Pipe and AsyncBufRead
        logger.write(Pipe::StdOut, Cursor::new(message)).await?;
        logger.write(Pipe::StdErr, Cursor::new(error_message)).await?;

        let res = fs::read_to_string(path)?;
        let logs: Vec<Value> = res.lines().map(|line| serde_json::from_str(line).unwrap()).collect();

        assert_eq!(logs.len(), 2);

        let first_log = &logs[0];
        assert_eq!(first_log["message"].as_str().unwrap(), "this is a line");
        assert_eq!(first_log["pipe"].as_str().unwrap(), "stdout");
        assert!(first_log["timestamp"].is_string());

        let second_log = &logs[1];
        assert_eq!(second_log["message"].as_str().unwrap(), "and another line");
        assert_eq!(second_log["pipe"].as_str().unwrap(), "stderr");
        assert!(second_log["timestamp"].is_string());

        Ok(())
    }
}


