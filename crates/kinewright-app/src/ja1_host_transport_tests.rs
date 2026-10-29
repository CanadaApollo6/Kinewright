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
        command: &'static str,
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
                let request = packet(&mut downstream);
                let header_end = request
                    .windows(4)
                    .position(|bytes| bytes == b"\r\n\r\n")
                    .unwrap()
                    + 4;
                let value: serde_json::Value =
                    serde_json::from_slice(&request[header_end..]).unwrap();
                let dropping = value
                    .pointer("/command/command")
                    .and_then(serde_json::Value::as_str)
                    == Some(command)
                    && !observed.swap(true, Ordering::AcqRel);
                if dropping && before {
                    continue;
                }
                let mut target = TcpStream::connect(upstream).unwrap();
                target.write_all(&request).unwrap();
                let response = packet(&mut target);
                if !dropping {
                    let _ = downstream.write_all(&response);
                }
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
        ReplyDropProxy::install(&mut self.client, &self.manual_client, command, before)
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
                command,
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
}
