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

pub(in crate::ja1_attachment) struct ReplyDropProxy {
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

impl HostAttachment {
    pub(in crate::ja1_attachment) fn drop_http_reply(
        &mut self,
        command: &'static str,
        before: bool,
    ) -> ReplyDropProxy {
        let upstream = self.client.endpoint().address;
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
        let mut bootstrap = self.client.bootstrap();
        bootstrap.endpoint.address = address;
        self.client = HostClient::connect(bootstrap.endpoint, bootstrap.auth, bootstrap.roots)
            .unwrap()
            .with_manual_editor(self.manual_client.clone());
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
