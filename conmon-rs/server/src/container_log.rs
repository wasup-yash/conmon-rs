use crate::{container_io::Pipe, cri_logger::CriLogger, json_logger::JsonLogger};
use anyhow::Result;
use capnp::struct_list::Reader;
use conmon_common::conmon_capnp::conmon::log_driver::{Owned, Type};
use futures::{FutureExt, future::join_all};
use std::sync::Arc;
use tokio::{io::AsyncBufRead, sync::RwLock};

pub type SharedContainerLog = Arc<RwLock<ContainerLog>>;

#[derive(Debug, Default)]
pub struct ContainerLog {
    drivers: Vec<LogDriver>,
}

#[derive(Debug)]
enum LogDriver {
    ContainerRuntimeInterface(CriLogger),
    Json(JsonLogger),
}

impl ContainerLog {
     /// Create a new default SharedContainerLog.
    pub fn new() -> SharedContainerLog {
        Arc::new(RwLock::new(Self::default()))
    }
    /// Create a new SharedContainerLog from an capnp owned reader.
    pub fn from(reader: Reader<Owned>) -> Result<SharedContainerLog> {
        let drivers = reader
            .iter()
            .map(|x| -> Result<_> {
                match x.get_type()? {
                    Type::ContainerRuntimeInterface => {
                        Ok(LogDriver::ContainerRuntimeInterface(CriLogger::new(
                            x.get_path()?,
                            if x.get_max_size() > 0 {
                                Some(x.get_max_size() as usize)
                            } else {
                                None
                            },
                        )?))
                    }
                    Type::Json => {
                        Ok(LogDriver::Json(JsonLogger::new(
                            x.get_path()?,
                            if x.get_max_size() > 0 {
                                Some(x.get_max_size() as usize)
                            } else {
                                None
                            },
                        )?))
                    }
                }
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Arc::new(RwLock::new(Self { drivers })))
    }
    /// Asynchronously initialize all loggers.
    pub async fn init(&mut self) -> Result<()> {
        join_all(
            self.drivers
                .iter_mut()
                .map(|x| match x {
                    LogDriver::ContainerRuntimeInterface(ref mut cri_logger) => cri_logger.init().boxed(),
                    LogDriver::Json(ref mut json_logger) => json_logger.init().boxed(),
                })
                .collect::<Vec<_>>(),
        )
        .await
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
        Ok(())
    }
      /// Reopen the container logs.
    pub async fn reopen(&mut self) -> Result<()> {
        join_all(
            self.drivers
                .iter_mut()
                .map(|x| match x {
                    LogDriver::ContainerRuntimeInterface(ref mut cri_logger) => cri_logger.reopen(),
                    LogDriver::Json(ref mut json_logger) => json_logger.reopen(),
                })
                .collect::<Vec<_>>(),
        )
        .await
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
        Ok(())
    }

    pub async fn write<T>(&mut self, pipe: Pipe, bytes: T) -> Result<()>
where
    T: AsyncBufRead + Unpin + Clone,
{
    let futures = self.drivers.iter_mut().map(|x| {
        async fn box_future<'a, T: AsyncBufRead + Unpin + Clone>(
            logger: &mut LogDriver,
            pipe: Pipe,
            bytes: T,
        ) -> Result<()> {
            match logger {
                LogDriver::ContainerRuntimeInterface(cri_logger) => cri_logger.write(pipe, bytes).await,
                LogDriver::Json(json_logger) => json_logger.write(pipe, bytes).await,
            }
        }

        box_future(x, pipe, bytes.clone())
    }).collect::<Vec<_>>();

    join_all(futures).await.into_iter().collect::<Result<Vec<_>>>()?;
    Ok(())
}
}

