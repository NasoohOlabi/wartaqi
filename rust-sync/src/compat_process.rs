use std::{
    io,
    process::Stdio,
    sync::{Arc, Mutex, Weak},
    time::Duration,
};

use tokio::{
    io::{AsyncBufReadExt, BufReader},
    process::{Child, Command},
    sync::Mutex as AsyncMutex,
};
use url::Url;

const IDLE: Duration = Duration::from_secs(90);
const STARTUP: Duration = Duration::from_secs(20);

#[derive(Clone)]
pub(crate) struct CompatProcess(Arc<Inner>);

struct Inner {
    state: Mutex<State>,
    startup: AsyncMutex<()>,
}

#[derive(Default)]
struct State {
    child: Option<Child>,
    url: Option<Url>,
    leases: usize,
    generation: u64,
}

pub(crate) struct CompatLease {
    inner: Arc<Inner>,
    url: Url,
}

impl CompatProcess {
    pub(crate) fn new() -> Self {
        Self(Arc::new(Inner {
            state: Mutex::new(State::default()),
            startup: AsyncMutex::new(()),
        }))
    }

    pub(crate) async fn acquire(&self) -> io::Result<CompatLease> {
        let _startup = self.0.startup.lock().await;
        {
            let mut state = self.0.state.lock().unwrap();
            if let Some(child) = state.child.as_mut()
                && child.try_wait()?.is_some()
            {
                state.child = None;
                state.url = None;
            }
            if let Some(url) = state.url.clone() {
                state.leases += 1;
                state.generation = state.generation.wrapping_add(1);
                return Ok(CompatLease {
                    inner: self.0.clone(),
                    url,
                });
            }
        }
        let (child, url) = tokio::time::timeout(STARTUP, start()).await.map_err(|_| {
            io::Error::new(
                io::ErrorKind::TimedOut,
                "compatibility server startup timed out",
            )
        })??;
        let mut state = self.0.state.lock().unwrap();
        state.child = Some(child);
        state.url = Some(url.clone());
        state.leases += 1;
        state.generation = state.generation.wrapping_add(1);
        Ok(CompatLease {
            inner: self.0.clone(),
            url,
        })
    }
}

impl CompatLease {
    pub(crate) fn url(&self) -> &Url {
        &self.url
    }
}

impl Drop for CompatLease {
    fn drop(&mut self) {
        let generation = {
            let mut state = self.inner.state.lock().unwrap();
            state.leases -= 1;
            state.generation = state.generation.wrapping_add(1);
            if state.leases != 0 {
                return;
            }
            state.generation
        };
        let weak: Weak<Inner> = Arc::downgrade(&self.inner);
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                tokio::time::sleep(IDLE).await;
                if let Some(inner) = weak.upgrade() {
                    let mut state = inner.state.lock().unwrap();
                    if state.leases == 0 && state.generation == generation {
                        state.child = None;
                        state.url = None;
                    }
                }
            });
        }
    }
}

async fn start() -> io::Result<(Child, Url)> {
    let mut child = Command::new("bun")
        .arg("server/compat.ts")
        .env_remove("PORT")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .kill_on_drop(true)
        .spawn()?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("missing compatibility stdout"))?;
    let mut stdout = BufReader::new(stdout);
    let mut line = String::new();
    stdout.read_line(&mut line).await?;
    let port = line
        .trim_end()
        .strip_prefix("WARTAQ_COMPAT_READY=")
        .and_then(|value| value.parse::<u16>().ok())
        .filter(|port| *port != 0)
        .ok_or_else(|| io::Error::other("invalid compatibility readiness marker"))?;
    if child.try_wait()?.is_some() {
        return Err(io::Error::other(
            "compatibility server exited during startup",
        ));
    }
    tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    if child.try_wait()?.is_some() {
        return Err(io::Error::other(
            "compatibility server exited during startup",
        ));
    }
    tokio::spawn(async move {
        let _ = tokio::io::copy(&mut stdout, &mut tokio::io::sink()).await;
    });
    let url = Url::parse(&format!("http://127.0.0.1:{port}/")).map_err(io::Error::other)?;
    Ok((child, url))
}
