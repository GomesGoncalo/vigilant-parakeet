use crate::control::node::ReplyType;
use anyhow::Result;
use futures::Future;
use std::sync::Arc;
use common::tun::Tun;
use uninit::uninit_array;

pub struct SessionParams {}
struct InnerSession {
    tun: Arc<Tun>,
}
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
                let buf = uninit_array![u8; 1500];
                let mut buf = unsafe { std::mem::transmute::<_, [u8; 1500]>(buf) };
                let n = tun.recv(&mut buf).await?;
                callable(buf, n).await
            }
            Self::ValidSession(session) => {
                todo!()
            }
        }
    }
}
