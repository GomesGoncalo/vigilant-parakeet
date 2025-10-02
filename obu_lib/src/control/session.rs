use super::node::ReplyType;
use anyhow::Result;
use common::tun::Tun;
use futures::Future;
use std::sync::Arc;

// Session support is under development - types are placeholders for future implementation
#[cfg_attr(not(test), allow(dead_code))]
pub struct SessionParams {}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) struct InnerSession {
    tun: Arc<Tun>,
}

#[cfg_attr(not(test), allow(dead_code))]
pub enum Session {
    NoSession(Arc<Tun>),
    ValidSession(InnerSession),
}

impl Session {
    pub fn new(tun: Arc<Tun>) -> Self {
        Self::NoSession(tun)
    }

    pub async fn process<Fut>(
        &self,
        callable: impl FnOnce([u8; 1500], usize) -> Fut,
    ) -> Result<Option<Vec<ReplyType>>>
    where
        Fut: Future<Output = Result<Option<Vec<ReplyType>>>>,
    {
        match self {
            Self::NoSession(tun) => {
                // allocate a zeroed buffer and read into it safely
                let mut buf: [u8; 1500] = [0u8; 1500];
                let n = tun.recv(&mut buf).await?;
                callable(buf, n).await
            }
            Self::ValidSession(_session) => {
                // session handling not implemented yet
                todo!()
            }
        }
    }
}
