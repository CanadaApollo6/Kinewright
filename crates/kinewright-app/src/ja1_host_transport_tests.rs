//! A real loopback proxy drops one HTTP request or reply, without bypassing App errors.
use super::*;
use std::{
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub(crate) struct ReplyDropProxy {
    stop: Arc<AtomicBool>,
    pub dropped: Arc<AtomicBool>,
    /// The commands dropped so far, in order; each is dropped at most once.
    pub lost: Arc<std::sync::Mutex<Vec<&'static str>>>,
    worker: Option<std::thread::JoinHandle<()>>,
}

fn packet(stream: &mut TcpStream) -> Vec<u8> {
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    let mut bytes = Vec::new();
    let mut byte = [0];
    while !bytes.ends_with(b"\r\n\r\n") {
        assert_eq!(stream.read(&mut byte).unwrap(), 1, "HTTP header");
        bytes.push(byte[0]);
    }
    let header = String::from_utf8(bytes.clone()).unwrap();
    let length: usize = header
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(':')?;
            key.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().unwrap())
        })
        .unwrap_or(0);
    let start = bytes.len();
    bytes.resize(start + length, 0);
    stream.read_exact(&mut bytes[start..]).unwrap();
    bytes
}

impl ReplyDropProxy {
    fn install(
        client: &mut HostClient,
        identity: &str,
        commands: Vec<&'static str>,
        before: bool,
    ) -> Self {
        let upstream = client.endpoint().address;
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let observed = Arc::clone(&dropped);
        let lost = Arc::new(std::sync::Mutex::new(Vec::new()));
        let losing = Arc::clone(&lost);
        let worker = std::thread::spawn(move || {
            while !stopping.load(Ordering::Acquire) {
                let (mut downstream, _) = match listener.accept() {
                    Ok(value) => value,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::park_timeout(Duration::from_millis(2));
                        continue;
                    }
                    Err(error) => panic!("proxy accept: {error}"),
                };
                // Each exchange runs on its own thread, so a background request
                // never waits behind another one.
                let (commands, losing, observed) =
                    (commands.clone(), Arc::clone(&losing), Arc::clone(&observed));
                std::thread::spawn(move || {
                    let request = packet(&mut downstream);
                    let header_end = request
                        .windows(4)
                        .position(|bytes| bytes == b"\r\n\r\n")
                        .unwrap()
                        + 4;
                    let value: Option<serde_json::Value> =
                        serde_json::from_slice(&request[header_end..]).ok();
                    let named = value
                        .as_ref()
                        .and_then(|value| value.pointer("/command/command"))
                        .and_then(serde_json::Value::as_str);
                    let mut losing = losing.lock().unwrap();
                    let dropping = commands
                        .iter()
                        .find(|command| Some(**command) == named && !losing.contains(*command))
                        .copied();
                    if let Some(command) = dropping {
                        losing.push(command);
                        observed.store(true, Ordering::Release);
                    }
                    drop(losing);
                    let dropping = dropping.is_some();
                    if dropping && before {
                        return;
                    }
                    let mut target = TcpStream::connect(upstream).unwrap();
                    target.write_all(&request).unwrap();
                    let response = packet(&mut target);
                    if !dropping {
                        let _ = downstream.write_all(&response);
                    }
                });
            }
        });
        let mut bootstrap = client.bootstrap();
        bootstrap.endpoint.address = address;
        *client = HostClient::connect(bootstrap.endpoint, bootstrap.auth, bootstrap.roots)
            .unwrap()
            .with_manual_editor(identity.to_owned());
        ReplyDropProxy {
            stop,
            dropped,
            lost,
            worker: Some(worker),
        }
    }
}

impl Drop for ReplyDropProxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.worker.take().unwrap().join().unwrap();
    }
}

impl Drop for HostAttachment {
    fn drop(&mut self) {
        // Test fixtures own every host they launch, including panic paths.
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl HostAttachment {
    pub(in crate::ja1_attachment) fn take_child_for_test(&mut self) -> std::process::Child {
        self.child.take().expect("owned fixture host")
    }
    pub(in crate::ja1_attachment) fn manual_identity_for_test(&self) -> String {
        self.manual_client.clone()
    }

    pub(in crate::ja1_attachment) fn drop_http_reply(
        &mut self,
        command: &'static str,
        before: bool,
    ) -> ReplyDropProxy {
        self.drop_http_replies(vec![command], before)
    }
    pub(in crate::ja1_attachment) fn drop_http_replies(
        &mut self,
        commands: Vec<&'static str>,
        before: bool,
    ) -> ReplyDropProxy {
        ReplyDropProxy::install(&mut self.client, &self.manual_client, commands, before)
    }
}
thread_local! {
    static LAUNCH_DROP: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
    static LAUNCH_PROXY: std::cell::RefCell<Option<ReplyDropProxy>> = const { std::cell::RefCell::new(None) };
}
pub(super) fn intercept_launch(mut client: HostClient, identity: &str) -> HostClient {
    if let Some(command) = LAUNCH_DROP.with(|enabled| enabled.replace(None)) {
        LAUNCH_PROXY.with_borrow_mut(|slot| {
            *slot = Some(ReplyDropProxy::install(
                &mut client,
                identity,
                vec![command],
                false,
            ));
        });
    }
    client
}
pub(in crate::ja1_attachment) fn drop_launch_reply<T>(
    command: &'static str,
    run: impl FnOnce() -> T,
) -> (T, ReplyDropProxy) {
    LAUNCH_DROP.with(|enabled| enabled.set(Some(command)));
    let result = run();
    let proxy = LAUNCH_PROXY
        .with_borrow_mut(Option::take)
        .expect("launch proxy");
    (result, proxy)
}

impl crate::ja1_attachment::Ja1Attachment {
    /// Drop one HTTP request or reply of `command` between this App and its host.
    pub(crate) fn drop_host_reply(
        &mut self,
        command: &'static str,
        before: bool,
    ) -> ReplyDropProxy {
        self.host
            .as_mut()
            .expect("launched host")
            .drop_http_reply(command, before)
    }
    /// Drop one HTTP reply of each of `commands`, in whatever order they come.
    pub(crate) fn drop_host_replies(&mut self, commands: Vec<&'static str>) -> ReplyDropProxy {
        self.host
            .as_mut()
            .expect("launched host")
            .drop_http_replies(commands, false)
    }
    /// Import media the way another editor of this host would: the host's
    /// inputs move, and this App learns the new scope only from a later status
    /// (S3 C11 fu1).
    pub(crate) fn import_elsewhere(&self, path: &std::path::Path) -> HostReply {
        let host = self.host.as_ref().expect("launched host");
        let snapshot = HostSnapshot::capture(self.owner.core()).unwrap();
        let asset_id = kinewright_core::AssetId(
            snapshot
                .history
                .document
                .media_pool
                .iter()
                .map(|asset| asset.id.0)
                .max()
                .unwrap_or(0)
                + 1,
        );
        host.client
            .request(HostCommand::PrepareOwnedMedia {
                client: host.manual_client.clone(),
                path: path.into(),
                asset_id,
                snapshot,
            })
            .unwrap()
    }
    /// The launched host process, so a test can end exactly that child.
    pub(crate) fn host_pid(&self) -> Option<u32> {
        self.host
            .as_ref()?
            .child
            .as_ref()
            .map(std::process::Child::id)
    }
}
