use crate::control::node::ReplyType;
use anyhow::Result;
use common::tun::Tun;
use futures::Future;
use std::sync::Arc;

#[allow(dead_code)]
pub struct SessionParams {}

#[allow(dead_code)]
pub(crate) struct InnerSession {
    tun: Arc<Tun>,
}

#[allow(dead_code)]
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
        callable: impl FnOnce(Arc<[u8]>, usize) -> Fut,
    ) -> Result<Option<Vec<ReplyType>>>
    where
        Fut: Future<Output = Result<Option<Vec<ReplyType>>>>,
    {
        match self {
            Self::NoSession(tun) => {
                // allocate a buffer, read into it, and pass as Arc for zero-copy
                let mut v = vec![0u8; 1500];
                let n = tun.recv(&mut v).await?;
                v.truncate(n);
                let arc: Arc<[u8]> = v.into_boxed_slice().into();
                callable(arc, n).await
            }
            Self::ValidSession(_session) => {
                // session handling not implemented yet
                todo!()
            }
        }
    }
}
