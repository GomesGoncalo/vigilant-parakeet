use anyhow::Result;
use common::tun::Tun;
use futures::Future;
use node_lib::control::node::ReplyType;
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
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn new(tun: Arc<Tun>) -> Self {
        Self::NoSession(tun)
    }

    #[cfg_attr(not(test), allow(dead_code))]
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn nosession_process_reads_from_tun() {
        let (a, b) = node_lib::test_helpers::util::mk_shim_pair();
        let tun = Arc::new(a);

        // spawn a task to send a packet from the peer
        let handle = tokio::spawn(async move {
            let _ = b.send_all(b"hello").await;
        });

        let s = Session::new(tun.clone());
        let res = s
            .process(|_buf, _n| async { Ok(Some(vec![])) })
            .await
            .expect("process ok");

        // callable returned Some(vec![]) so result should be Some
        assert!(res.is_some());
        handle.await.expect("peer task");
    }
}
